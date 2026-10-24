//! Frame composition: header, tab bar, body, status line, help overlay.

pub mod tabs;
pub mod tiles;
pub mod widgets;

use crate::app::{App, Tab};
use crate::theme::Theme;
use ratatui::backend::TestBackend;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Clear, Paragraph};
use ratatui::{Frame, Terminal};

/// The smallest frame that still says something: below this `too_small` speaks.
pub fn min_size() -> (u16, u16) {
    (24, 6)
}

/// A row the body needs more than the tab bar does. At nine rows the tabs cost
/// the difference between one usable panel and none.
pub fn shows_tabbar(area: Rect) -> bool {
    area.height >= 12
}

fn too_small(f: &mut Frame, area: Rect, app: &App) {
    let t = &app.theme;
    let (mw, mh) = min_size();
    let number = |value: u16, need: u16| {
        Span::styled(
            value.to_string(),
            Style::default().fg(if value < need { t.danger } else { t.fg }),
        )
    };
    // A 20-column window cannot hold "Needed: Width = 24  Height = 6"; say the
    // same thing in the room that exists instead of truncating it.
    let roomy = area.width >= 32 && area.height >= 5;
    let mut lines = vec![Line::from(Span::styled(
        if roomy {
            "routwatch: terminal too small"
        } else {
            "too small"
        },
        Style::default().fg(t.fg).add_modifier(Modifier::BOLD),
    ))];
    if roomy {
        lines.push(Line::from(vec![
            Span::styled("Width = ", Style::default().fg(t.dim)),
            number(area.width, mw),
            Span::styled("   Height = ", Style::default().fg(t.dim)),
            number(area.height, mh),
        ]));
        lines.push(Line::from(format!("Needed: Width = {mw}  Height = {mh}")));
        lines.push(Line::from(Span::styled(
            "80x24 is comfortable; 120x40 is roomy",
            Style::default().fg(t.dim),
        )));
    } else {
        lines.push(Line::from(vec![
            Span::raw("W="),
            number(area.width, mw),
            Span::raw(" H="),
            number(area.height, mh),
        ]));
        lines.push(Line::from(format!("Needed: {mw}x{mh}")));
    }
    let top = area.y + area.height.saturating_sub(lines.len() as u16) / 2;
    for (offset, line) in lines.into_iter().enumerate() {
        let y = top + offset as u16;
        if y >= area.bottom() {
            break;
        }
        f.render_widget(
            Paragraph::new(line).alignment(ratatui::layout::Alignment::Center),
            Rect {
                y,
                height: 1,
                ..area
            },
        );
    }
}

pub fn draw(f: &mut Frame, app: &App) {
    let area = f.area();
    let (mw, mh) = min_size();
    if area.width < mw || area.height < mh {
        too_small(f, area, app);
        return;
    }
    let (header, tabbar, body, status) = if shows_tabbar(area) {
        let [h, tb, b, s] = Layout::vertical([
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Min(3),
            Constraint::Length(1),
        ])
        .areas(area);
        (h, Some(tb), b, s)
    } else {
        let [h, b, s] = Layout::vertical([
            Constraint::Length(1),
            Constraint::Min(4),
            Constraint::Length(1),
        ])
        .areas(area);
        (h, None, b, s)
    };

    draw_header(f, header, app);
    if let Some(tabbar) = tabbar {
        draw_tabbar(f, tabbar, app);
    }
    tabs::body(f, body, app);
    if app.input.is_some() {
        draw_input(f, status, app);
    } else {
        draw_status(f, status, app);
    }

    if app.help {
        draw_help(f, area, &app.theme);
    }
    if app.events_open {
        draw_events(f, area, app);
    }

    // `sky` and `paper` own a panel fill. Widgets leave most cells with
    // `Reset`, so the fill is applied last: it only replaces cells nobody
    // deliberately painted (selection bars keep their own background).
    if app.theme.paints_bg() {
        let bg = app.theme.bg;
        let buf = f.buffer_mut();
        for y in area.top()..area.bottom() {
            for x in area.left()..area.right() {
                let cell = &mut buf[(x, y)];
                if cell.bg == ratatui::style::Color::Reset {
                    cell.bg = bg;
                }
            }
        }
    }
    if !matches!(app.write, crate::write::WriteState::Idle) {
        draw_write_modal(f, area, app);
    }
    if app.settings_open {
        draw_settings(f, area, app);
    }
}

/// In-app settings: appearance and polling only — this panel never touches the
/// router. `w` persists the values into config.toml at mode 0600.
fn draw_settings(f: &mut Frame, area: Rect, app: &App) {
    let t = &app.theme;
    let rows = vec![
        (
            "theme",
            format!(
                "{} ({}/{})",
                app.theme.id,
                routwatch_index_of(&app.theme.id),
                crate::theme::names().len()
            ),
            "t cycles · system/ansi = terminal",
        ),
        ("view", app.view.name().to_string(), "v cycles"),
        ("graph", app.graph.name().to_string(), "g cycles"),
        ("refresh", format!("{} ms", app.refresh_ms), "[ / ] adjust"),
        ("budget", format!("{:.0} GB", app.budget_gb), "- / + adjust"),
        (
            "report",
            if app.report_identity {
                "full"
            } else {
                "redacted"
            }
            .to_string(),
            "i identity · e writes it",
        ),
    ];
    let w = 56.min(area.width);
    let h = (rows.len() as u16 + 5).min(area.height);
    let rect = Rect {
        x: area.x + (area.width.saturating_sub(w)) / 2,
        y: area.y + (area.height.saturating_sub(h)) / 2,
        width: w,
        height: h,
    };
    f.render_widget(Clear, rect);
    let mut lines: Vec<Line> = rows
        .into_iter()
        .map(|(label, value, hint)| {
            Line::from(vec![
                Span::styled(format!(" {:<9}", label), Style::default().fg(t.dim)),
                Span::styled(format!("{:<16}", value), Style::default().fg(t.fg)),
                Span::styled(hint, Style::default().fg(t.accent)),
            ])
        })
        .collect();
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        " w save to config.toml · Esc / S close",
        Style::default().fg(t.dim),
    )));
    lines.push(Line::from(Span::styled(
        " appearance only — no router setting is changed here",
        Style::default().fg(t.dim),
    )));
    f.render_widget(
        Paragraph::new(lines).block(
            Block::bordered()
                .title(Span::styled(
                    " settings ",
                    Style::default().fg(t.title).add_modifier(Modifier::BOLD),
                ))
                .border_style(Style::default().fg(t.accent)),
        ),
        rect,
    );
}

/// The write confirmation / result modal. One intent at a time, always modal:
/// nothing else in the UI can be triggered while it is open.
fn draw_write_modal(f: &mut Frame, area: Rect, app: &App) {
    use crate::write::WriteState;
    let t = &app.theme;
    let (title, body, colour) = match &app.write {
        WriteState::Idle => return,
        WriteState::Confirming(intent) => {
            use crate::write::WriteIntent;
            let p = app.preflight().unwrap_or(crate::preflight::Preflight {
                rows: Vec::new(),
                warning: None,
            });
            let title = match intent {
                WriteIntent::ApplyBands { .. } => " apply bands? ",
                WriteIntent::SetDns { .. } => " set dns? ",
                WriteIntent::ResetDns => " reset dns? ",
            };
            let mut body = vec![intent.label(), String::new()];
            for (key, value) in &p.rows {
                body.push(format!("  {key:<12} {value}"));
            }
            if let Some(warning) = &p.warning {
                body.push(String::new());
                body.push(warning.clone());
            }
            body.push(String::new());
            body.push("y confirm   ·   n / Esc cancel".into());
            (title, body, t.warn)
        }
        WriteState::Pending { .. } | WriteState::Verifying { .. } => (
            " working ",
            vec![
                app.write.label(),
                String::new(),
                "waiting for the router…".into(),
            ],
            t.accent,
        ),
        WriteState::Success { note, .. } => (
            " applied ",
            vec![note.clone(), String::new(), "Enter / Esc to dismiss".into()],
            t.ok,
        ),
        WriteState::Failed { err, .. } => (
            " failed ",
            vec![err.clone(), String::new(), "Enter / Esc to dismiss".into()],
            t.danger,
        ),
    };
    let w = 68.min(area.width);
    let h = (body.len() as u16 + 3).min(area.height);
    let rect = Rect {
        x: area.x + (area.width.saturating_sub(w)) / 2,
        y: area.y + (area.height.saturating_sub(h)) / 2,
        width: w,
        height: h,
    };
    f.render_widget(Clear, rect);
    let lines: Vec<Line> = body
        .into_iter()
        .map(|l| Line::from(Span::styled(l, Style::default().fg(t.fg))))
        .collect();
    f.render_widget(
        Paragraph::new(lines).block(
            Block::bordered()
                .title(Span::styled(
                    title,
                    Style::default().fg(colour).add_modifier(Modifier::BOLD),
                ))
                .border_style(Style::default().fg(colour)),
        ),
        rect,
    );
}

fn draw_header(f: &mut Frame, area: Rect, app: &App) {
    let t = &app.theme;
    let plmn = app
        .snap
        .as_ref()
        .map(|s| format!("{} · {}", s.plmn.name, s.plmn.numeric))
        .unwrap_or_else(|| "connecting…".into());
    let net = app
        .snap
        .as_ref()
        .map(|s| {
            format!(
                "{} · ENDC {}",
                s.connection.net_type,
                if s.connection.endc { "on" } else { "off" }
            )
        })
        .unwrap_or_default();

    // The state word already says "live"; repeating it here would read
    // "● live · live 0s". Only staleness needs its own word.
    let (age_text, age_style) = match app.age() {
        Some(a) => {
            let secs = a.as_secs();
            if app.stale() {
                (format!("stale {secs}s"), Style::default().fg(t.warn))
            } else {
                (format!("{secs}s"), Style::default().fg(t.ok))
            }
        }
        None => ("waiting".to_string(), Style::default().fg(t.dim)),
    };

    // The state comes first and is the last thing dropped: "restarting" tells
    // you why the numbers are frozen, "poll 2000ms" does not.
    let state = app.session.short();
    let state_style = match &app.session {
        crate::app::SessionState::Live => Style::default().fg(t.ok),
        crate::app::SessionState::Disconnected(_) => Style::default().fg(t.danger),
        crate::app::SessionState::Restarting => Style::default().fg(t.warn),
        _ => Style::default().fg(t.accent),
    };
    let mut right = vec![Span::styled(format!("● {state}"), state_style)];
    if app.paused {
        right.push(Span::styled(" · paused", Style::default().fg(t.warn)));
    }
    right.push(Span::styled(format!(" · {age_text}"), age_style));
    // The endpoint being read right now, and how far the scan got. Hidden when
    // no scan is in flight, so an idle header stays quiet.
    if let Some(phase) = app.current_phase() {
        let (done, total) = app.phase_progress();
        let tick = if done > 0 { "✓ " } else { "" };
        right.push(Span::styled(
            format!(" · {tick}{} {done}/{total}", phase.label()),
            Style::default().fg(t.accent),
        ));
    }
    let mut right_width: u16 = right
        .iter()
        .map(|span| span.content.chars().count() as u16)
        .sum();
    let poll = format!(" · poll {}ms ", app.last_poll_ms);
    // Keep at least 34 columns for the left side: the PLMN is worth more than
    // the polling interval.
    if right_width + poll.chars().count() as u16 + 34 <= area.width {
        right.push(Span::styled(poll.clone(), Style::default().fg(t.dim)));
        right_width += poll.chars().count() as u16;
    }
    // The right side is drawn over the same row, so the left side has to know
    // how much room it may take — otherwise " routwatch " and "waiting" merge
    // into "routwaiting" on a narrow terminal.
    let room = area.width.saturating_sub(right_width + 1);
    let brand = 11u16;
    let mut left = Vec::new();
    if room >= brand {
        left.push(Span::styled(
            " routwatch ",
            Style::default().fg(t.accent).add_modifier(Modifier::BOLD),
        ));
        let detail = format!("· {plmn} · {net}");
        if room > brand + 6 {
            left.push(Span::styled(
                truncate(&detail, room.saturating_sub(brand) as usize),
                Style::default().fg(t.fg),
            ));
        }
    }
    f.render_widget(Paragraph::new(Line::from(left)), area);
    if area.width >= 30 {
        f.render_widget(
            Paragraph::new(Line::from(right)).alignment(ratatui::layout::Alignment::Right),
            area,
        );
    }
}

fn draw_tabbar(f: &mut Frame, area: Rect, app: &App) {
    let t = &app.theme;
    let mut spans: Vec<Span> = Vec::new();
    // Nine abbreviated labels need 76 columns (measured): below that, show
    // where you are and how to move instead of a clipped list of nine.
    if area.width < 76 {
        let index = app.tab.index() + 1;
        let label = format!(
            " {index}/{} {} · Tab next ",
            Tab::ALL.len(),
            app.tab.short()
        );
        f.render_widget(
            Paragraph::new(Line::from(Span::styled(
                label,
                Style::default()
                    .fg(active_text_colour(t))
                    .bg(t.accent)
                    .add_modifier(Modifier::BOLD),
            ))),
            area,
        );
        return;
    }
    // Eight full titles need ~96 columns; below that, abbreviate instead of clipping.
    let narrow = area.width < 96;
    for (i, tab) in Tab::ALL.iter().enumerate() {
        let active = *tab == app.tab;
        let name = if narrow { tab.short() } else { tab.title() };
        let label = format!(" {i} {name} ", i = i + 1);
        let style = if active {
            Style::default()
                .fg(active_text_colour(t))
                .bg(t.accent)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(t.dim)
        };
        spans.push(Span::styled(label, style));
        if i + 1 < Tab::ALL.len() {
            spans.push(Span::styled("│", Style::default().fg(t.border)));
        }
    }
    if !narrow {
        spans.push(Span::styled(
            format!("   {}/{}", app.view.name(), app.graph.name()),
            Style::default().fg(t.dim),
        ));
    }
    f.render_widget(Paragraph::new(Line::from(spans)), area);
}

fn draw_status(f: &mut Frame, area: Rect, app: &App) {
    let t = &app.theme;
    // Keys outrank every other fact: below 40 columns they get the whole row,
    // because a clipped "q quit" is a key the user cannot find.
    let keys = " · q quit · ? help";
    if area.width < 60 {
        // Narrow terminals: one fact plus the keys, never a truncated key line.
        let room = (area.width as usize).saturating_sub(keys.chars().count() + 1);
        let mut spans = Vec::new();
        if area.width >= 34 {
            let (text, style) = match app.live_notice() {
                Some(note) => (truncate(note, room), Style::default().fg(t.accent)),
                None => (
                    truncate(app.tab.short(), room),
                    Style::default().fg(t.accent),
                ),
            };
            spans.push(Span::styled(format!(" {text}"), style));
        }
        spans.push(Span::styled(keys, Style::default().fg(t.dim)));
        f.render_widget(Paragraph::new(Line::from(spans)), area);
        return;
    }
    // The session state lives in the header now; this row is keys and notes.
    let mut spans = vec![
        Span::styled(
            format!(" {} ", app.tab.short()),
            Style::default().fg(t.accent),
        ),
        Span::styled(
            format!(
                "· {} · refresh {}ms",
                app.snap
                    .as_ref()
                    .map(|s| s.router.clone())
                    .unwrap_or_default(),
                app.refresh_ms
            ),
            Style::default().fg(t.dim),
        ),
    ];
    let mut used = spans
        .iter()
        .map(|s| s.content.chars().count())
        .sum::<usize>();
    if let Some(note) = app.live_notice() {
        let text = format!(" · {}", truncate(note, area.width as usize / 2));
        used += text.chars().count();
        spans.push(Span::styled(text, Style::default().fg(t.accent)));
    } else if let Some(err) = app.error_log.front() {
        let text = format!(" · {}", truncate(err, area.width as usize / 2));
        used += text.chars().count();
        spans.push(Span::styled(text, Style::default().fg(t.danger)));
    }

    // This section's own keys sit next to the global ones. They are the first
    // thing to give up space on a narrow terminal (never `q quit · ? help`).
    // `i events` is discoverability, not a requirement: it is advertised only
    // when the line can still hold it and still leave room for a hint.
    const GLOBAL_TAIL: &str = " · q quit · ? help";
    let wide_tail = " · i events · q quit · ? help";
    let tail = if (area.width as usize) > used + wide_tail.chars().count() + 6 {
        wide_tail
    } else {
        GLOBAL_TAIL
    };
    let available = (area.width as usize)
        .saturating_sub(used + tail.chars().count())
        .saturating_sub(1);
    let hint = app.tab.hints();
    if !hint.is_empty() && available >= 20 {
        // " · " costs three columns; the hint gets what is left, no more.
        let room = available.saturating_sub(3);
        spans.push(Span::styled(
            format!(" · {}", hint_fit(hint, room)),
            Style::default().fg(t.accent),
        ));
    }
    spans.push(Span::styled(tail, Style::default().fg(t.dim)));
    f.render_widget(Paragraph::new(Line::from(spans)), area);
}

/// One-line text prompt (ping host / DNS pair) drawn in place of the status bar.
fn draw_input(f: &mut Frame, area: Rect, app: &App) {
    let t = &app.theme;
    let Some(input) = &app.input else { return };
    let hint = match input.kind {
        crate::app::InputKind::PingHost => "Enter run · Esc cancel",
        crate::app::InputKind::Dns => "Enter confirm · Esc cancel · format primary,secondary",
        crate::app::InputKind::RestartConfirm => {
            "Enter runs it · Esc cancels · this drops the connection"
        }
    };
    let line = Line::from(vec![
        Span::styled(
            format!(" {}: ", input.label),
            Style::default().fg(t.accent).add_modifier(Modifier::BOLD),
        ),
        Span::styled(input.value.clone(), Style::default().fg(t.fg)),
        Span::styled("█", Style::default().fg(t.accent)),
        Span::styled(format!("   {hint}"), Style::default().fg(t.dim)),
    ]);
    f.render_widget(Paragraph::new(line), area);
}

/// Fit a `·`-separated hint list into `room` columns, cutting between entries
/// so it never ends mid-word (`… · o opt…` reads as a typo).
fn hint_fit(s: &str, room: usize) -> String {
    if s.chars().count() <= room {
        return s.to_string();
    }
    let mut kept = String::new();
    for part in s.split(" · ") {
        let candidate = if kept.is_empty() {
            part.to_string()
        } else {
            format!("{kept} · {part}")
        };
        if candidate.chars().count() + 1 > room {
            break;
        }
        kept = candidate;
    }
    if kept.is_empty() {
        truncate(s, room)
    } else {
        format!("{kept}…")
    }
}

fn truncate(s: &str, n: usize) -> String {
    if s.chars().count() <= n {
        s.to_string()
    } else {
        s.chars().take(n.saturating_sub(1)).collect::<String>() + "…"
    }
}

/// The event feed: a window over `app.event_log`, newest line at the top.
///
/// The selection is counted from the newest entry (`events_back`), so an event
/// arriving while the reader is stopped here cannot shift the line they are on.
fn draw_events(f: &mut Frame, area: Rect, app: &App) {
    let t = &app.theme;
    let w = area.width.saturating_sub(8).clamp(16, 110).min(area.width);
    let h = (area.height.saturating_sub(6)).max(8).min(area.height);
    let rect = Rect {
        x: area.x + (area.width.saturating_sub(w)) / 2,
        y: area.y + (area.height.saturating_sub(h)) / 2,
        width: w,
        height: h,
    };
    f.render_widget(Clear, rect);

    // A bordered box loses two rows; the footer and its blank line lose two more.
    let inner = h.saturating_sub(2) as usize;
    let visible = inner.saturating_sub(2);
    let selected = app.events_back;
    let mut lines: Vec<Line> = Vec::new();
    for (i, entry) in app
        .event_log
        .iter()
        .skip(selected)
        .take(visible)
        .enumerate()
    {
        let colour = match entry.level {
            crate::app::LogLevel::Error => t.danger,
            crate::app::LogLevel::Warn => t.warn,
            crate::app::LogLevel::Info => t.fg,
        };
        let marker = if i == 0 { "›" } else { " " };
        lines.push(Line::from(vec![
            Span::styled(marker, Style::default().fg(t.accent)),
            Span::styled(format!(" {:>5}s ", entry.at), Style::default().fg(t.dim)),
            Span::styled(entry.msg.clone(), Style::default().fg(colour)),
        ]));
    }
    if lines.is_empty() {
        lines.push(Line::from(Span::styled(
            " no events yet ",
            Style::default().fg(t.dim),
        )));
    }
    // The footer belongs to the bottom edge, not to the last event.
    while lines.len() + 2 < inner {
        lines.push(Line::from(""));
    }
    lines.push(Line::from(""));
    // `events_back > 0` is already a held position, even if nothing toggled the
    // pause: saying "following" there would be a lie.
    let state = if app.events_paused || app.events_back > 0 {
        "paused"
    } else {
        "following"
    };
    let footer = if w >= 64 {
        format!(
            " {} events · {state} · j/k · space pause · i close ",
            app.event_log.len()
        )
    } else {
        format!(" {} events · {state} · i close ", app.event_log.len())
    };
    lines.push(Line::from(Span::styled(footer, Style::default().fg(t.dim))));
    f.render_widget(
        Paragraph::new(lines).block(
            Block::bordered()
                .title(Span::styled(
                    " events ",
                    Style::default().fg(t.title).add_modifier(Modifier::BOLD),
                ))
                .border_style(Style::default().fg(t.accent)),
        ),
        rect,
    );
}

fn draw_help(f: &mut Frame, area: Rect, theme: &Theme) {
    let rows = [
        ("1..7 / Tab", "switch tab"),
        ("j k / ↑ ↓", "move selection"),
        ("i", "event feed: j/k reads back, space pauses, Esc closes"),
        ("space / a", "bands: toggle / apply (confirm with y)"),
        ("m / o", "bands: network mode / NR option"),
        (
            "n / s / x",
            "bands: next merge preset / save the ticked merge / delete it",
        ),
        (
            "1-9 / Tab",
            "jump to a section from ANY view (Lite/Dense return to full)",
        ),
        (
            "d / z",
            "DNS tab (8) + traffic: type servers / back to DHCP",
        ),
        (
            "8 / j k / Enter",
            "DNS tab: pick a preset and apply (then y)",
        ),
        ("9", "Signal tab: gauge dashboard"),
        ("t", "network: ping a host from this machine"),
        (
            "s / c",
            "clients: sort / copy MAC (bands tab: s saves a merge)",
        ),
        ("e / R", "SMS/Device: read inbox, SIM state, data switch"),
        ("x / b", "device: dump JSON / restart router (type REBOOT)"),
        ("S / ,", "settings overlay (appearance, w saves)"),
        ("v / V", "cycle view: full → gauges → lite → dense"),
        ("l / L", "toggle lite (returns to the view you came from)"),
        ("g", "cycle graph: dots → bars → braille"),
        ("r", "refresh now"),
        ("p", "pause / resume polling"),
        ("?", "this help"),
        ("q / Ctrl+C", "quit (logs out)"),
    ];
    let h = rows.len() as u16 + 4;
    let w = 52.min(area.width);
    let rect = Rect {
        x: area.x + (area.width.saturating_sub(w)) / 2,
        y: area.y + (area.height.saturating_sub(h)) / 2,
        width: w,
        height: h.min(area.height),
    };
    f.render_widget(Clear, rect);
    let mut lines: Vec<Line> = rows
        .iter()
        .map(|(k, d)| {
            Line::from(vec![
                Span::styled(
                    format!(" {k:<12}"),
                    Style::default()
                        .fg(theme.accent)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::styled(*d, Style::default().fg(theme.fg)),
            ])
        })
        .collect();
    lines.push(Line::from(Span::styled(
        " read-only build: no router writes are enabled",
        Style::default().fg(theme.dim),
    )));
    f.render_widget(
        Paragraph::new(lines).block(
            Block::bordered()
                .title(Span::styled(
                    " keys ",
                    Style::default()
                        .fg(theme.title)
                        .add_modifier(Modifier::BOLD),
                ))
                .border_style(Style::default().fg(theme.accent)),
        ),
        rect,
    );
}

/// Render one frame to text — the screenshot substitute for tests and for
/// `routwatch preview`, which lets a human see the layout without a TTY.
pub fn render_to_string(app: &App, width: u16, height: u16) -> String {
    let backend = TestBackend::new(width, height);
    let mut terminal = Terminal::new(backend).expect("test terminal");
    terminal.draw(|f| draw(f, app)).expect("draw");
    let buffer = terminal.backend().buffer();
    let mut out = String::new();
    for y in 0..height {
        for x in 0..width {
            out.push_str(buffer[(x, y)].symbol());
        }
        out.push('\n');
    }
    out
}

/// Render one frame with real SGR escapes, like a terminal would receive.
///
/// `render_to_string` deliberately drops styling so tests can assert on text;
/// this one keeps it, which is what README screenshots and `preview --ansi`
/// need. Colors follow the same rules ratatui's crossterm backend uses:
/// named ANSI slots, `38;5;n` for indexed, `38;2;r;g;b` for RGB, `Reset`
/// falls back to the terminal defaults.
pub fn render_ansi(app: &App, width: u16, height: u16) -> String {
    let backend = TestBackend::new(width, height);
    let mut terminal = Terminal::new(backend).expect("test terminal");
    terminal.draw(|f| draw(f, app)).expect("draw");
    let buffer = terminal.backend().buffer();

    let mut out = String::new();
    for y in 0..height {
        let mut current: Option<Style> = None;
        for x in 0..width {
            let cell = &buffer[(x, y)];
            let style = Style::default()
                .fg(cell.fg)
                .bg(cell.bg)
                .add_modifier(cell.modifier);
            if current != Some(style) {
                out.push_str("\x1b[0m");
                out.push_str(&sgr(style));
                current = Some(style);
            }
            out.push_str(cell.symbol());
        }
        out.push_str("\x1b[0m\n");
    }
    out
}

fn sgr(style: Style) -> String {
    let mut codes: Vec<String> = Vec::new();
    if style.add_modifier.contains(Modifier::BOLD) {
        codes.push("1".into());
    }
    if style.add_modifier.contains(Modifier::DIM) {
        codes.push("2".into());
    }
    if style.add_modifier.contains(Modifier::ITALIC) {
        codes.push("3".into());
    }
    if style.add_modifier.contains(Modifier::UNDERLINED) {
        codes.push("4".into());
    }
    if style.add_modifier.contains(Modifier::REVERSED) {
        codes.push("7".into());
    }
    let (fg, bg) = match (style.fg, style.bg) {
        (Some(f), Some(b)) => (Some(f), Some(b)),
        _ => (style.fg, style.bg),
    };
    if let Some(fg) = fg {
        codes.push(color_code(fg, false));
    }
    if let Some(bg) = bg {
        codes.push(color_code(bg, true));
    }
    if codes.is_empty() {
        String::new()
    } else {
        format!("\x1b[{}m", codes.join(";"))
    }
}

fn color_code(c: ratatui::style::Color, background: bool) -> String {
    use ratatui::style::Color as C;
    let base = if background { 40 } else { 30 };
    let bright = if background { 100 } else { 90 };
    let simple = |n: u8| (base + n).to_string();
    let bright_code = |n: u8| (bright + n).to_string();
    match c {
        C::Reset => {
            if background {
                "49".into()
            } else {
                "39".into()
            }
        }
        C::Black => simple(0),
        C::Red => simple(1),
        C::Green => simple(2),
        C::Yellow => simple(3),
        C::Blue => simple(4),
        C::Magenta => simple(5),
        C::Cyan => simple(6),
        C::Gray => simple(7),
        C::White => simple(7),
        C::DarkGray => bright_code(0),
        C::LightRed => bright_code(1),
        C::LightGreen => bright_code(2),
        C::LightYellow => bright_code(3),
        C::LightBlue => bright_code(4),
        C::LightMagenta => bright_code(5),
        C::LightCyan => bright_code(6),
        C::Indexed(n) => format!("{};5;{n}", if background { 48 } else { 38 }),
        C::Rgb(r, g, b) => format!("{};2;{r};{g};{b}", if background { 48 } else { 38 }),
    }
}

/// 1-based position of the active theme in the family cycle, for the settings row.
fn routwatch_index_of(id: &str) -> usize {
    crate::theme::names()
        .iter()
        .position(|n| *n == id)
        .map(|i| i + 1)
        .unwrap_or(0)
}

/// Text colour that reads on top of the accent colour (active tab, cursor
/// cells). This is the palette's own `text_inverse` slot — netwatch and
/// syswatch do exactly this rather than guessing per theme.
pub fn active_text_colour(theme: &Theme) -> ratatui::style::Color {
    theme.inverse
}

pub fn tab_from_key(c: char) -> Option<Tab> {
    match c {
        '1'..='9' => Some(Tab::from_index((c as u8 - b'1') as usize)),
        _ => None,
    }
}

pub fn view_note(app: &App) -> String {
    app.view.name().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::View;
    use crate::theme::GraphStyle;

    fn fixture_app() -> App {
        let snap: crate::model::Snapshot =
            serde_json::from_str(include_str!("../../fixtures/sample-snapshot.json"))
                .expect("fixture snapshot");
        let mut app = App::new(
            Theme::get("terminal"),
            GraphStyle::Dots,
            View::Full,
            100,
            2000,
        );
        app.on_snapshot(snap, 432);
        app
    }

    #[test]
    fn dashboard_renders_expected_fields() {
        let app = fixture_app();
        let text = render_to_string(&app, 100, 30);
        assert!(text.contains("routwatch"), "header missing\n{text}");
        assert!(text.contains("RSRP"));
        assert!(text.contains("-95"), "frozen fixture shows RSRP -95");
        assert!(text.contains("B1"));
        assert!(text.contains("Test Net"));
        assert!(text.contains("CLIENTS") || text.contains("Dashboard"));
    }

    #[test]
    fn every_tab_renders_without_panic() {
        let mut app = fixture_app();
        for tab in Tab::ALL {
            app.tab = tab;
            let text = render_to_string(&app, 100, 30);
            assert!(text.contains("routwatch"), "header missing on {:?}", tab);
            assert!(
                text.to_lowercase().contains(&tab.title().to_lowercase()),
                "tab title missing on {:?}",
                tab
            );
        }
    }

    #[test]
    fn the_too_small_screen_names_the_numbers_it_needs() {
        let app = fixture_app();
        let text = render_to_string(&app, 20, 5);
        assert!(text.contains("too small"), "no verdict line\n{text}");
        assert!(text.contains("Needed"), "no requirement line\n{text}");
        assert!(text.contains("24"), "width requirement missing\n{text}");
        assert!(text.contains("24x6"), "requirement missing\n{text}");
    }

    #[test]
    fn the_feed_names_its_pause_state() {
        let mut app = fixture_app();
        app.event_log.clear();
        app.events_open = true;
        app.log(crate::app::LogLevel::Info, "scan started".to_string());
        let live = render_to_string(&app, 100, 30);
        assert!(live.contains("following"), "a live feed says so:\n{live}");
        app.events_paused = true;
        let held = render_to_string(&app, 100, 30);
        assert!(held.contains("paused"), "a held feed says paused:\n{held}");
        assert!(held.contains("1 events"), "the count is missing:\n{held}");
    }

    #[test]
    fn the_feed_marks_the_line_it_is_reading() {
        let mut app = fixture_app();
        app.event_log.clear();
        app.events_open = true;
        app.events_paused = true;
        for i in 0..5 {
            app.log(crate::app::LogLevel::Info, format!("line {i}"));
        }
        // Newest first: line 4, line 3, line 2 … Reading two lines back starts
        // the window on line 2, with the cursor on that row.
        app.event_select(2);
        let text = render_to_string(&app, 100, 30);
        let rows: Vec<&str> = text.lines().filter(|line| line.contains("line ")).collect();
        assert!(rows[0].contains('›'), "no cursor mark: {:?}", rows[0]);
        assert!(
            rows[0].contains("line 2"),
            "wrong line selected: {:?}",
            rows[0]
        );
        assert!(
            rows[1].contains("line 1"),
            "order is not newest-first: {rows:?}"
        );

        // Following shows the newest line at the top of the window.
        app.events_paused = false;
        app.events_back = 0;
        let text = render_to_string(&app, 100, 30);
        let first = text
            .lines()
            .find(|line| line.contains("line "))
            .unwrap_or_default();
        assert!(
            first.contains("line 4"),
            "newest line not on top: {first:?}"
        );
    }

    #[test]
    fn the_feed_fits_a_forty_column_terminal() {
        let mut app = fixture_app();
        app.events_open = true;
        app.log(
            crate::app::LogLevel::Warn,
            "a line long enough that it must be clipped by the window".to_string(),
        );
        let text = render_to_string(&app, 40, 20);
        for line in text.lines() {
            assert!(
                line.chars().count() <= 40,
                "the feed overflowed 40 columns: {line:?}"
            );
        }
        assert!(text.contains("events"), "the feed did not render:\n{text}");
    }

    #[test]
    fn a_short_terminal_drops_the_tab_bar_and_keeps_the_status_line() {
        let app = fixture_app();
        let text = render_to_string(&app, 100, 9);
        assert!(!text.contains("Bands"), "tab bar should be gone:\n{text}");
        assert!(text.contains("q quit"), "status line must survive:\n{text}");
    }

    #[test]
    fn a_tall_terminal_keeps_the_tab_bar() {
        let app = fixture_app();
        let text = render_to_string(&app, 100, 24);
        assert!(text.contains("Bands"), "tab bar missing at 100x24:\n{text}");
    }

    /// Every section, at the smallest supported sizes: nothing may panic, and
    /// the keys must stay on screen in all of them.
    #[test]
    fn every_section_survives_the_smallest_supported_size() {
        let mut app = fixture_app();
        for (w, h) in [(24, 6), (40, 10), (50, 45)] {
            for tab in Tab::ALL {
                app.select_tab(tab);
                let text = render_to_string(&app, w, h);
                assert!(
                    text.contains("q quit") || text.contains("Tab"),
                    "{tab:?} at {w}x{h} lost its key hints:\n{text}"
                );
            }
        }
    }

    /// Landscape, portrait, small, medium and large: the chrome must survive and
    /// nothing may panic or lose its last line.
    #[test]
    fn the_frame_survives_the_sizes_it_claims_to_support() {
        let app = fixture_app();
        for (w, h) in [
            (24, 6),
            (40, 10),
            (50, 45),
            (60, 20),
            (80, 24),
            (120, 40),
            (160, 24),
            (200, 60),
        ] {
            let text = render_to_string(&app, w, h);
            let lines: Vec<&str> = text.lines().collect();
            assert_eq!(lines.len(), h as usize, "{w}x{h} lost rows");
            assert!(
                text.contains("q quit"),
                "{w}x{h} lost the status line:\n{text}"
            );
            assert!(
                text.chars()
                    .any(|c| c == '│' || c == '─' || c.is_alphanumeric()),
                "{w}x{h} rendered nothing"
            );
        }
    }

    #[test]
    fn views_and_small_sizes_render() {
        let mut app = fixture_app();
        for view in [View::Full, View::Lite, View::Dense] {
            app.view = view;
            let text = render_to_string(&app, 80, 24);
            assert!(!text.trim().is_empty(), "{:?} rendered empty", view);
        }
        app.view = View::Full;
        let tiny = render_to_string(&app, 20, 5);
        assert!(tiny.contains("too small"));
    }

    #[test]
    fn bands_tab_shows_selection_and_mask() {
        let mut app = fixture_app();
        app.tab = Tab::Bands;
        app.bands_supported = vec![1, 3, 5, 8, 20];
        app.bands_selected = vec![1, 3, 5];
        let text = render_to_string(&app, 100, 22);
        assert!(
            text.contains("[B1]"),
            "selected band must be bracketed\n{text}"
        );
        assert!(text.contains("mask 0x15"), "mask preview missing\n{text}");
        assert!(text.contains("4G only") || text.contains("Auto"));
    }

    #[test]
    fn confirm_modal_renders_and_cancels() {
        let mut app = fixture_app();
        app.tab = Tab::Bands;
        app.bands_supported = vec![1, 3];
        app.bands_selected = vec![1, 3];
        assert!(app.begin_confirm());
        let text = render_to_string(&app, 100, 26);
        assert!(text.contains("apply bands?"), "modal title missing\n{text}");
        assert!(text.contains("y confirm"));
        assert!(text.contains("offline"), "link-drop warning missing");
        app.cancel_confirm();
        assert!(matches!(app.write, crate::write::WriteState::Idle));
    }

    #[test]
    fn the_header_names_the_session_state_not_only_the_age() {
        let mut app = fixture_app();
        app.session = crate::app::SessionState::Restarting;
        let text = render_to_string(&app, 120, 30);
        let head = text.lines().next().unwrap_or_default();
        assert!(
            head.to_lowercase().contains("restarting"),
            "header says nothing about the state: {head}"
        );
        assert!(text.contains("q quit"), "keys stay in the status line");
    }

    #[test]
    fn a_paused_session_says_paused_in_the_header() {
        let mut app = fixture_app();
        app.paused = true;
        let text = render_to_string(&app, 120, 30);
        let head = text.lines().next().unwrap_or_default();
        assert!(head.contains("paused"), "header hid the pause: {head}");
    }

    #[test]
    fn the_header_keeps_the_state_when_the_line_is_short() {
        let mut app = fixture_app();
        app.session = crate::app::SessionState::Recovering {
            attempt: 3,
            wait_s: 8,
        };
        let text = render_to_string(&app, 60, 20);
        let head = text.lines().next().unwrap_or_default();
        assert!(
            head.contains("recovering #3"),
            "state was dropped before the clock: {head}"
        );
        assert!(!head.contains("poll"), "poll should yield first: {head}");
    }

    #[test]
    fn a_running_scan_shows_its_phase_and_count_in_the_header() {
        use crate::headless::{Phase, PhaseState};
        let mut app = fixture_app();
        for phase in Phase::ALL.iter().take(4) {
            app.on_phase(*phase, PhaseState::Done);
        }
        app.on_phase(Phase::Device, PhaseState::Running);
        let text = render_to_string(&app, 120, 30);
        let head = text.lines().next().unwrap_or_default();
        assert!(head.contains("device"), "phase missing from header: {head}");
        assert!(head.contains("4/10"), "count missing from header: {head}");
        assert!(
            head.contains('✓'),
            "no tick for the finished phases: {head}"
        );
    }

    #[test]
    fn a_running_scan_fits_the_header_at_eighty_columns() {
        use crate::headless::{Phase, PhaseState};
        let mut app = fixture_app();
        app.session = crate::app::SessionState::Live;
        for phase in Phase::ALL.iter().take(5) {
            app.on_phase(*phase, PhaseState::Done);
        }
        app.on_phase(Phase::Clients, PhaseState::Running);
        let text = render_to_string(&app, 80, 30);
        let head = text.lines().next().unwrap_or_default();
        assert!(head.contains("clients"), "phase lost at 80 cols: {head}");
        assert!(head.contains("5/10"), "count lost at 80 cols: {head}");
        let state_at = head.find('●').expect("state marker");
        assert!(state_at > 0, "state lost at 80 cols: {head}");
        assert_eq!(
            head[..state_at].chars().last(),
            Some(' '),
            "left and right ran into each other: {head}"
        );
    }

    #[test]
    fn a_short_dashboard_drops_the_instrument_band() {
        let mut app = fixture_app();
        let mut snap = crate::model::Snapshot::default();
        snap.signal.rsrp_dbm = Some(-95.0);
        app.snap = Some(snap);
        let text = render_to_string(&app, 100, 24);
        assert!(
            !text.contains("SIGNAL MAP"),
            "band shown without rows for it: {text}"
        );
    }

    #[test]
    fn an_idle_header_says_nothing_about_phases() {
        let app = fixture_app();
        let head = render_to_string(&app, 120, 30);
        let head = head.lines().next().unwrap_or_default();
        assert!(!head.contains("/10"), "idle header shows a scan: {head}");
    }

    #[test]
    fn the_status_line_drops_the_session_and_names_the_section() {
        let app = fixture_app();
        let text = render_to_string(&app, 120, 30);
        let status = text.lines().last().unwrap_or_default();
        assert!(status.contains("Dash"), "section missing: {status}");
        assert!(
            !status.contains("connecting") && !status.contains("live"),
            "session state duplicated in the status bar: {status}"
        );
    }

    #[test]
    fn the_settings_overlay_names_the_report_mode() {
        let mut app = fixture_app();
        app.settings_open = true;
        let text = render_to_string(&app, 100, 30);
        assert!(text.contains("report"), "no report row\n{text}");
        assert!(text.contains("redacted"), "mode not named\n{text}");
        app.report_identity = true;
        let full = render_to_string(&app, 100, 30);
        assert!(full.contains("full"));
    }

    #[test]
    fn every_dashboard_hint_advertises_the_report_key_but_sms_keeps_e_for_extras() {
        use crate::app::Tab;
        assert!(Tab::Dashboard.hints().contains("e report"));
        assert!(Tab::Bands.hints().contains("e report"));
        assert!(!Tab::Sms.hints().contains("e report"));
        assert!(!Tab::Device.hints().contains("e report"));
    }

    #[test]
    fn the_confirm_modal_shows_the_after_state_and_the_outage() {
        let mut app = fixture_app();
        app.tab = Tab::Bands;
        app.bands_selected = vec![1, 3];
        assert!(app.begin_confirm());
        let text = render_to_string(&app, 100, 30);
        assert!(text.contains("bands after"), "no after-state\n{text}");
        assert!(
            text.contains("B1,B3"),
            "after-state is not the selection\n{text}"
        );
        assert!(text.contains("offline"), "outage line missing\n{text}");
    }

    #[test]
    fn busy_write_swallows_further_edits() {
        let mut app = fixture_app();
        app.bands_supported = vec![1, 3];
        app.bands_selected = vec![1];
        app.settle_write(crate::write::WriteState::Pending {
            intent: app.band_intent(),
            since: std::time::Instant::now(),
        });
        assert!(app.write.is_busy());
        app.band_toggle();
        assert_eq!(
            app.bands_selected,
            vec![1],
            "toggle must be ignored while writing"
        );
        assert!(!app.begin_confirm(), "a second confirm must be refused");
    }

    #[test]
    fn traffic_tab_renders_the_dns_panel() {
        let mut app = fixture_app();
        app.tab = Tab::Traffic;
        let text = render_to_string(&app, 100, 26);
        assert!(text.contains("DNS"), "dns panel missing\n{text}");
        assert!(text.contains("203.0.113.53"), "primary DNS missing");
        assert!(text.contains("d edit DNS"));
    }

    #[test]
    fn network_tab_renders_ping_panel_and_result() {
        let mut app = fixture_app();
        app.tab = Tab::Network;
        let text = render_to_string(&app, 100, 26);
        assert!(text.contains("PING"));
        assert!(text.contains("no test yet"));

        app.set_ping(crate::worker::PingOutcome {
            host: "1.1.1.1".into(),
            sent: 3,
            received: 3,
            loss_pct: 0,
            avg_ms: Some(12.4),
            ok: true,
            detail: "3/3 replies · 0% loss".into(),
        });
        let text = render_to_string(&app, 100, 26);
        assert!(text.contains("1.1.1.1"));
        assert!(text.contains("12.4 ms"));
        assert!(text.contains("3/3 replies"));
    }

    #[test]
    fn clients_tab_shows_sort_key() {
        let mut app = fixture_app();
        app.tab = Tab::Clients;
        let text = render_to_string(&app, 100, 20);
        assert!(text.contains("sort name"));
        app.sort_next();
        let text = render_to_string(&app, 100, 20);
        assert!(text.contains("sort ip"));
    }

    #[test]
    fn dns_tab_lists_presets_and_current_state() {
        let mut app = fixture_app();
        app.tab = Tab::Dns;
        let text = render_to_string(&app, 100, 28);
        assert!(
            text.contains("Automatic (DHCP)"),
            "preset list missing\n{text}"
        );
        assert!(text.contains("Cloudflare") && text.contains("1.1.1.1 / 1.0.0.1"));
        assert!(text.contains("Custom"), "custom row missing\n{text}");
        assert!(text.contains("type any two IPv4 servers"));
        assert!(text.contains("current"), "current DNS row missing");
        assert!(text.contains("verified by reading dhcp/settings back"));

        app.dns_preset = 2;
        let text = render_to_string(&app, 100, 28);
        assert!(text.contains("Google"));
    }

    #[test]
    fn input_line_replaces_the_status_bar() {
        let mut app = fixture_app();
        app.open_input(crate::app::InputKind::Dns);
        let text = render_to_string(&app, 100, 24);
        assert!(
            text.contains("dns primary,secondary"),
            "prompt label missing\n{text}"
        );
        assert!(text.contains("Enter confirm"));
        assert!(
            !text.contains("q quit"),
            "status bar must yield to the prompt"
        );
    }

    #[test]
    fn sms_tab_states_the_blocker_honestly() {
        let mut app = fixture_app();
        app.tab = Tab::Sms;
        let text = render_to_string(&app, 100, 26);
        assert!(text.contains("COUNTERS"));
        assert!(text.contains("press R to read the inbox"));

        app.on_extras(crate::model::DeviceExtras {
            sms_error: Some("permission denied (100005)".into()),
            ..Default::default()
        });
        let text = render_to_string(&app, 100, 26);
        assert!(
            text.contains("blocked: permission denied (100005)"),
            "\n{text}"
        );
        assert!(text.contains("192.168.8.1"));
        assert!(
            !text.contains("inbox is empty"),
            "must never pretend it is empty"
        );
    }

    #[test]
    fn device_tab_shows_extras_and_event_log() {
        let mut app = fixture_app();
        app.tab = Tab::Device;
        app.on_extras(crate::model::DeviceExtras {
            pin: Some(crate::model::PinStatus {
                sim_state: Some("257".into()),
                pin_state: Some("258".into()),
                pin_retries: Some("3".into()),
                puk_retries: Some("10".into()),
            }),
            mobile_data: Some(true),
            ..Default::default()
        });
        app.log(crate::app::LogLevel::Info, "session live · Test Net · LTE");
        let text = render_to_string(&app, 100, 30);
        assert!(text.contains("EVENTS"), "event log panel missing\n{text}");
        assert!(text.contains("session live"));
        assert!(text.contains("mobile") && text.contains("data on"));
        assert!(text.contains("state 257"));
    }

    #[test]
    fn settings_overlay_renders() {
        let mut app = fixture_app();
        app.settings_open = true;
        let text = render_to_string(&app, 100, 28);
        assert!(text.contains("settings"));
        assert!(text.contains("w save to config.toml"));
        assert!(text.contains("no router setting is changed here"));
    }

    #[test]
    fn all_three_signal_graphs_paint_with_history() {
        let mut app = fixture_app();
        app.tab = Tab::Signal;
        // 60 samples so each series has a visible trace
        for i in 0..60 {
            let mut snap = app.snap.clone().unwrap();
            snap.signal.rsrp_dbm = Some(-100.0 + (i % 20) as f32);
            snap.signal.rsrq_db = Some(-15.0 + (i % 8) as f32);
            snap.signal.sinr_db = Some(2.0 + (i % 15) as f32);
            app.on_snapshot(snap, 1);
        }
        let text = render_to_string(&app, 120, 34);
        for label in ["RSRP", "RSRQ", "SINR"] {
            assert!(text.contains(label), "{label} label missing");
        }
        let graph_block = text
            .split("SIGNAL GRAPHS")
            .nth(1)
            .expect("graphs panel missing");
        let painted = graph_block
            .chars()
            .filter(|c| ('\u{2800}'..='\u{28ff}').contains(c) && *c != '\u{2800}')
            .count();
        println!("painted braille dots in graphs: {painted}");
        println!("{graph_block}");
        assert!(painted > 30, "expected three traces, painted={painted}");
    }

    #[test]
    fn signal_tab_shows_gauges_in_full_but_yields_to_other_views() {
        let mut app = fixture_app();
        app.tab = Tab::Signal;

        app.view = View::Full;
        let full = render_to_string(&app, 120, 32);
        assert!(
            full.contains("SIGNAL · 4G") || full.contains("SIGNAL · 5G"),
            "\n{full}"
        );
        assert!(full.contains("SIGNAL GRAPHS"));

        // Light and dense views must be able to take the screen over, otherwise
        // `v` looks dead while tab 9 is open.
        app.view = View::Lite;
        let lite = render_to_string(&app, 120, 32);
        assert!(
            !lite.contains("SIGNAL GRAPHS"),
            "lite must replace the dashboard\n{lite}"
        );
        assert_ne!(lite, full);

        app.view = View::Dense;
        let dense = render_to_string(&app, 120, 32);
        assert!(
            !dense.contains("SIGNAL GRAPHS"),
            "dense must replace the dashboard"
        );
        assert_ne!(dense, lite);
    }

    #[test]
    fn gauges_view_is_left_by_choosing_a_section() {
        let mut app = fixture_app();
        app.view = View::Gauges;
        app.tab = Tab::Bands;
        let gauges = render_to_string(&app, 120, 30);
        assert!(
            gauges.contains("SIGNAL · 4G"),
            "the gauges view owns the body"
        );

        app.select_tab(Tab::Traffic);
        assert_eq!(
            app.view,
            View::Full,
            "a section must escape the gauges view"
        );
        let traffic = render_to_string(&app, 120, 30);
        assert!(!traffic.contains("SIGNAL GRAPHS"), "\n{traffic}");
        assert_ne!(gauges, traffic);
    }

    #[test]
    fn gauges_view_renders_wide_and_compact() {
        let mut app = fixture_app();
        app.view = View::Gauges;
        let wide = render_to_string(&app, 120, 34);
        assert!(wide.contains("SIGNAL · 4G"), "\n{wide}");
        assert!(wide.contains("RSRP") && wide.contains("RSRQ") && wide.contains("SINR"));
        assert!(wide.contains("SPEED"));
        assert!(wide.contains("SIGNAL GRAPHS"));

        let compact = render_to_string(&app, 80, 24);
        assert!(compact.contains("SIGNAL"), "\n{compact}");
        assert!(!compact.trim().is_empty());
    }

    #[test]
    fn gauges_view_switches_to_bars_without_nr_metrics() {
        let mut app = fixture_app();
        app.view = View::Gauges;
        if let Some(s) = app.snap.as_mut() {
            s.connection.endc = true;
            s.connection.nr_bars = 4;
        }
        let text = render_to_string(&app, 120, 34);
        assert!(text.contains("5G (bars only)"), "\n{text}");
        assert!(text.contains("no NR RSRP/SINR values"));
        assert!(text.contains("4/5"));
    }

    #[test]
    fn a_digit_escapes_lite_and_draws_the_section() {
        let mut app = fixture_app();
        app.view = View::Lite;
        let lite = render_to_string(&app, 120, 30);
        app.select_tab(Tab::Bands);
        let after = render_to_string(&app, 120, 30);
        assert_eq!(app.view, View::Full);
        assert_ne!(lite, after, "the body must change, not just the tab bar");
        assert!(after.contains("BANDS"), "\n{after}");
    }

    #[test]
    fn bands_tab_shows_merge_presets_and_the_merge_line() {
        let mut app = fixture_app();
        app.select_tab(Tab::Bands);
        let text = render_to_string(&app, 120, 30);
        assert!(text.contains("presets"), "preset row missing\n{text}");
        assert!(text.contains("B1+B3"), "ready-made merge missing\n{text}");
        assert!(
            text.contains("B1 + B3"),
            "the ticked merge must be spelled out\n{text}"
        );
        assert!(
            text.contains("s save"),
            "the save key must be discoverable\n{text}"
        );

        // an unsupported band in a preset is marked, not hidden
        app.band_presets = vec![crate::config::BandPreset::new(&[1, 3, 66])];
        app.bands_supported = vec![1, 3];
        let text = render_to_string(&app, 120, 30);
        assert!(
            text.contains("B1+B3+B66!"),
            "unsupported preset unmarked\n{text}"
        );
    }

    #[test]
    fn status_line_carries_the_current_tabs_own_keys() {
        let mut app = fixture_app();
        let cases = [
            (Tab::Bands, "s save"),
            (Tab::Traffic, "d edit DNS"),
            (Tab::Network, "t ping host"),
            (Tab::Clients, "c copy mac"),
            (Tab::Dns, "Enter apply"),
            (Tab::Device, "x dump json"),
            (Tab::Signal, "v view"),
        ];
        for (tab, needle) in cases {
            app.select_tab(tab);
            let text = render_to_string(&app, 160, 30);
            let status = text.lines().last().unwrap_or_default();
            assert!(
                status.contains(needle),
                "{tab:?} status bar is missing {needle:?}: {status}"
            );
            assert!(
                status.contains("q quit"),
                "{tab:?}: global keys must survive"
            );
        }

        // A narrow terminal drops keys, never the global ones.
        app.select_tab(Tab::Bands);
        let status = render_to_string(&app, 60, 24)
            .lines()
            .last()
            .unwrap_or_default()
            .to_string();
        assert!(
            status.contains("q quit") && status.contains("? help"),
            "{status}"
        );
        assert!(
            !status.contains("a apply"),
            "hints must yield before globals: {status}"
        );
    }

    #[test]
    fn help_documents_case_free_keys() {
        let mut app = fixture_app();
        app.help = true;
        let text = render_to_string(&app, 120, 34);
        for needle in ["v / V", "l / L", "S / ,", "jump to a section from ANY view"] {
            assert!(text.contains(needle), "help is missing {needle:?}\n{text}");
        }
    }

    #[test]
    fn painted_themes_fill_the_panel_and_keep_selection_backgrounds() {
        let mut app = fixture_app();
        let sky = crate::theme::by_name("sky");
        app.theme = sky.clone();
        let backend = TestBackend::new(60, 12);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|f| draw(f, &app)).unwrap();
        let buf = terminal.backend().buffer();
        assert_eq!(
            buf[(0u16, 0u16)].bg,
            sky.bg,
            "an unpainted cell must take the panel fill"
        );
        // The active-tab cell keeps its own accent background — the fill must
        // not steamroll deliberate backgrounds.
        let kept = (0..12u16)
            .flat_map(|y| (0..60u16).map(move |x| (x, y)))
            .any(|(x, y)| {
                buf[(x, y)].bg != sky.bg && buf[(x, y)].bg != ratatui::style::Color::Reset
            });
        assert!(
            kept,
            "the accent highlight disappeared under the panel fill"
        );

        // A transparent theme leaves every background to the terminal.
        app.theme = crate::theme::by_name("dark");
        let mut terminal = Terminal::new(TestBackend::new(60, 12)).unwrap();
        terminal.draw(|f| draw(f, &app)).unwrap();
        let buf = terminal.backend().buffer();
        assert_eq!(
            buf[(0u16, 0u16)].bg,
            ratatui::style::Color::Reset,
            "dark must not paint over the terminal"
        );
    }

    #[test]
    fn hints_are_cut_between_entries_never_mid_word() {
        assert_eq!(
            hint_fit("space tick · j/k · n preset", 40),
            "space tick · j/k · n preset"
        );
        assert_eq!(hint_fit("space tick · j/k · n preset", 12), "space tick…");
        // the ellipsis is inside the budget, so the line never overflows
        assert!(hint_fit("space tick · j/k · n preset", 12).chars().count() <= 12);
        assert_eq!(
            hint_fit("space tick · j/k", 3),
            "sp…",
            "no entry fits ⇒ plain truncation"
        );
    }

    #[test]
    fn tab_keys_map_to_tabs() {
        assert_eq!(tab_from_key('1'), Some(Tab::Dashboard));
        assert_eq!(tab_from_key('7'), Some(Tab::Device));
        assert_eq!(tab_from_key('8'), Some(Tab::Dns));
        assert_eq!(tab_from_key('9'), Some(Tab::Signal));
        assert_eq!(tab_from_key('0'), None);
    }
}
