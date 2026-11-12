//! Frame composition: header, tab bar, body, status line, help overlay.

pub mod chart;
pub mod help;
pub mod tabs;
pub mod tiles;
pub mod widgets;

use crate::app::{App, Tab};
use crate::theme::Theme;
use ratatui::backend::TestBackend;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Clear, Paragraph, Wrap};
use ratatui::{Frame, Terminal};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

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
        draw_help(f, area, &app.theme, app.allow_write);
    }
    if app.events_open {
        draw_events(f, area, app);
    }

    if !matches!(app.write, crate::write::WriteState::Idle) {
        draw_write_modal(f, area, app);
    }
    // Before settings, so an open settings overlay still wins.
    if app.reset_confirm {
        draw_reset_modal(f, area, app);
    }
    if app.settings_open {
        draw_settings(f, area, app);
    }

    // `sky` and `paper` own a panel fill. Applied last so it also covers the
    // modal's `Clear` cells, which otherwise fall back to the terminal default
    // and leave dark text on a dark terminal.
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
}

/// In-app settings: appearance and polling only - this panel never touches the
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
        " appearance only - no router setting is changed here",
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

/// Local, not a router write: no preflight rows, no read-back. One sentence and
/// two answers.
fn draw_reset_modal(f: &mut Frame, area: Rect, app: &App) {
    let t = &app.theme;
    let w = area.width.min(54);
    let h = 7.min(area.height);
    let rect = Rect {
        x: area.x + (area.width - w) / 2,
        y: area.y + (area.height - h) / 2,
        width: w,
        height: h,
    };
    f.render_widget(Clear, rect);
    let used = app
        .snap
        .as_ref()
        .map(|s| crate::usage::used(&s.traffic, s.quota.as_ref(), &app.usage))
        .unwrap_or(0);
    let lines = vec![
        Line::from(Span::styled(
            "reset the traffic budget?",
            Style::default().fg(t.fg).add_modifier(Modifier::BOLD),
        )),
        Line::from(""),
        Line::from(Span::styled(
            format!(
                "{} counted of {:.0} GB",
                widgets::bytes(used),
                app.budget_gb
            ),
            Style::default().fg(t.dim),
        )),
        Line::from(""),
        Line::from(Span::styled(
            "the month counters are not touched",
            Style::default().fg(t.dim),
        )),
        Line::from(Span::styled(
            "y reset   ·   n / Esc cancel",
            Style::default().fg(t.dim),
        )),
    ];
    f.render_widget(
        Paragraph::new(lines)
            .wrap(Wrap { trim: true })
            .block(tabs::panel("BUDGET".into(), t)),
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
    let inner_w = w.saturating_sub(2);
    let lines: Vec<Line> = body
        .into_iter()
        .map(|l| Line::from(Span::styled(l, Style::default().fg(t.fg))))
        .collect();
    // The paragraph decides the height: `line_count` on the same width it will be
    // drawn at. `body.len()` was the bug - one long reason is one body line and
    // three drawn lines, and the third was cut away by the border.
    let para = Paragraph::new(lines).wrap(Wrap { trim: false }).block(
        Block::bordered()
            .title(Span::styled(
                title,
                Style::default().fg(colour).add_modifier(Modifier::BOLD),
            ))
            .border_style(Style::default().fg(colour)),
    );
    let h = (para.line_count(inner_w) as u16).min(area.height);
    let rect = Rect {
        x: area.x + (area.width.saturating_sub(w)) / 2,
        y: area.y + (area.height.saturating_sub(h)) / 2,
        width: w,
        height: h,
    };
    f.render_widget(Clear, rect);
    f.render_widget(para, rect);
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
        .map(|span| UnicodeWidthStr::width(span.content.as_ref()) as u16)
        .sum();
    let poll = format!(" · poll {}ms ", app.last_poll_ms);
    // Keep at least 34 columns for the left side: the PLMN is worth more than
    // the polling interval.
    if right_width + UnicodeWidthStr::width(poll.as_str()) as u16 + 34 <= area.width {
        right.push(Span::styled(poll.clone(), Style::default().fg(t.dim)));
        right_width += UnicodeWidthStr::width(poll.as_str()) as u16;
    }
    // The right side is drawn over the same row, so the left side has to know
    // how much room it may take - otherwise " routwatch " and "waiting" merge
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
    let left_w: u16 = left
        .iter()
        .map(|span| UnicodeWidthStr::width(span.content.as_ref()) as u16)
        .sum();
    f.render_widget(Paragraph::new(Line::from(left)), area);
    // Show the state whenever it actually fits: the old `>= 30` check hid it on
    // 24..29 columns even when the text had room.
    if (area.width as usize) > left_w as usize + right_width as usize {
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
/// so it never ends mid-word (`… · o nr…` reads as a typo).
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
    if UnicodeWidthStr::width(s) <= n {
        return s.to_string();
    }
    let mut out = String::new();
    let mut w = 0usize;
    let room = n.saturating_sub(1);
    for ch in s.chars() {
        let cw = UnicodeWidthChar::width(ch).unwrap_or(0);
        if w + cw > room {
            break;
        }
        out.push(ch);
        w += cw;
    }
    out + "…"
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

fn draw_help(f: &mut Frame, area: Rect, theme: &Theme, allow_write: bool) {
    help::draw(f, area, theme, allow_write);
}

/// Render one frame to text - the screenshot substitute for tests and for
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
/// cells). This is the palette's own `text_inverse` slot - netwatch and
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

    #[test]
    fn the_signal_graphs_reach_the_panel_edge() {
        let mut app = fixture_app();
        app.tab = crate::app::Tab::Signal;
        for i in 0..60 {
            app.history_rsrp.push_back(-80.0 - (i as f64 % 20.0));
        }
        // Two widths: the trace must follow the panel, not a fixed gutter.
        for width in [130u16, 100, 80] {
            let text = render_to_string(&app, width, 40);
            let lines: Vec<&str> = text.lines().collect();
            let start = lines
                .iter()
                .position(|l| l.contains("SIGNAL GRAPHS"))
                .expect("graphs panel");
            // The baseline row of the first series is always inked, so one of the
            // trace rows must carry ink in the panel's very last cell.
            let reaches_edge = lines
                .iter()
                .skip(start + 1)
                .take(16)
                .filter_map(|l| l.strip_prefix('│').and_then(|x| x.strip_suffix('│')))
                .any(|inner| {
                    inner
                        .chars()
                        .next_back()
                        .map(|c| ('\u{2800}'..='\u{28ff}').contains(&c))
                        .unwrap_or(false)
                });
            assert!(
                reaches_edge,
                "the newest sample must paint the last cell at {width}"
            );
        }
    }

    #[test]
    fn the_dashboard_waits_for_its_first_sample() {
        let mut app = fixture_app();
        app.history_dl.clear();
        app.history_ul.clear();
        let frame = crate::ui::render_to_string(&app, 120, 40);
        assert!(
            frame.contains("waiting for the first sample"),
            "an empty plot must say so"
        );
        assert!(
            !frame.contains("last 1s"),
            "no sample means no window: {frame}"
        );
    }

    #[test]
    fn the_dashboard_names_its_window_and_draws_an_axis() {
        let mut app = App::new(
            crate::theme::by_name("dark"),
            GraphStyle::Dots,
            View::Full,
            100,
            2000,
        );
        app.snap = Some(
            serde_json::from_str(include_str!("../../fixtures/sample-snapshot.json")).unwrap(),
        );
        for i in 0..120 {
            app.history_dl.push_back(1_000_000.0 + i as f64);
            app.history_ul.push_back(100_000.0);
        }
        let frame = crate::ui::render_to_string(&app, 120, 40);
        assert!(
            frame.contains("THROUGHPUT · dots · last 240s"),
            "the title names the window actually drawn"
        );
        assert!(frame.contains("now"), "the x-axis is drawn");
        assert!(
            frame.contains("peak "),
            "the meta line carries peak and average"
        );
        assert!(!frame.contains("120 samples"), "the old claim is gone");
    }

    #[test]
    fn a_short_history_keeps_every_bar_one_cell_wide() {
        let mut app = App::new(
            crate::theme::by_name("dark"),
            GraphStyle::Dots,
            View::Full,
            100,
            2000,
        );
        app.snap = Some(
            serde_json::from_str(include_str!("../../fixtures/sample-snapshot.json")).unwrap(),
        );
        for i in 0..15 {
            app.history_dl
                .push_back(if i == 14 { 4_000_000.0 } else { 1_000_000.0 });
            app.history_ul.push_back(100_000.0);
        }
        let frame = crate::ui::render_to_string(&app, 120, 40);
        let lines: Vec<&str> = frame.lines().collect();
        let meta = lines
            .iter()
            .position(|l| l.contains("peak "))
            .expect("throughput meta row");
        let top = lines[meta + 1];
        let inked = top
            .chars()
            .filter(|c| ('\u{2800}'..='\u{28ff}').contains(c) && *c != '\u{2800}')
            .count();
        assert_eq!(
            inked, 1,
            "one sample is one cell wide, not a stretched block: {top:?}"
        );
        assert!(
            frame.contains("-30s") && frame.contains("now"),
            "a 15-cell plot still names its window"
        );
    }

    #[test]
    fn a_single_sample_says_collecting_and_draws_one_mark() {
        let mut app = App::new(
            crate::theme::by_name("dark"),
            GraphStyle::Dots,
            View::Full,
            100,
            2000,
        );
        app.snap = Some(
            serde_json::from_str(include_str!("../../fixtures/sample-snapshot.json")).unwrap(),
        );
        app.history_dl.push_back(4_000_000.0);
        app.history_ul.push_back(200_000.0);
        let frame = crate::ui::render_to_string(&app, 120, 40);
        assert!(
            frame.contains("THROUGHPUT · dots · 1 sample, collecting"),
            "one sample is a reading, not a window"
        );
        assert!(
            !frame.contains("last 2s"),
            "no window is claimed before one exists"
        );
        assert!(
            frame.contains("collecting…"),
            "the axis says what is happening instead of naming a span"
        );
    }

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
    fn the_signal_graphs_keep_their_vertical_range() {
        let ink = |app: &App| -> usize {
            let text = render_to_string(app, 130, 40);
            let tail = text
                .split_once("SIGNAL GRAPHS")
                .map(|(_, rest)| rest.to_string())
                .unwrap_or_default();
            tail.chars()
                .filter(|c| ('\u{2801}'..='\u{28ff}').contains(c))
                .count()
        };
        let mut floor = fixture_app();
        floor.tab = crate::app::Tab::Signal;
        floor.history_rsrp = (0..60).map(|_| -140.0).collect();
        let mut middle = fixture_app();
        middle.tab = crate::app::Tab::Signal;
        middle.history_rsrp = (0..60).map(|_| -92.0).collect();
        assert!(
            ink(&middle) > ink(&floor),
            "a mid-domain RSRP must paint more rows than the floor"
        );
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
        assert!(text.contains("→ 0x15"), "mask preview missing\n{text}");
        assert!(text.contains("4G only") || text.contains("Auto"));
    }

    #[test]
    fn bands_grid_columns_line_up_across_classes() {
        let mut app = fixture_app();
        app.select_tab(Tab::Bands);
        app.bands_supported = vec![1, 3, 5, 8, 20, 28, 32, 34, 39, 41];
        app.bands_selected = vec![1, 20];
        let text = render_to_string(&app, 100, 24);
        // Structural filter, never a word search: a grid row is the only line
        // whose class column (chars 10..16) holds one of the four padded labels.
        // A `warn` line mentions "high" too and must never enter this check.
        let grid: Vec<String> = text
            .lines()
            .filter(|l| {
                let chars: Vec<char> = l.chars().collect();
                if chars.len() <= 16 {
                    return false;
                }
                let cls: String = chars[10..16].iter().collect();
                ["low   ", "mid   ", "high  ", "other "].contains(&cls.as_str())
            })
            .map(|l| l.to_string())
            .collect();
        assert!(grid.len() >= 3, "three classes, three rows\n{text}");
        for line in &grid {
            let chars: Vec<char> = line.chars().collect();
            for (i, ch) in chars.iter().enumerate() {
                let starts_cell = *ch == '[' || (*ch == 'B' && i > 0 && chars[i - 1] != '[');
                if starts_cell {
                    assert!(
                        i >= 16 && (i - 16) % 6 == 0,
                        "band token at char {i} is off the 6-column grid: {line}"
                    );
                }
            }
        }
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

    /// A firmware without `wlan/host-list` gave `clients: []`, and the panel
    /// drew that as a real zero under a "CLIENTS · 0" title. A fact nobody
    /// measured must not look like a measurement.
    #[test]
    fn clients_tab_says_when_the_firmware_hides_the_list() {
        let mut app = fixture_app();
        app.tab = Tab::Clients;
        let snap = app.snap.as_mut().expect("fixture snapshot");
        snap.clients.clear();
        snap.unsupported = vec!["clients".into()];

        let text = render_to_string(&app, 100, 20);
        assert!(
            text.contains("not exposed by this firmware"),
            "the reason is missing:\n{text}"
        );
        assert!(
            !text.contains("CLIENTS · 0"),
            "an unmeasured list is still titled as zero:\n{text}"
        );

        // The plain empty case keeps its own wording: no hosts is not a refusal.
        app.snap.as_mut().expect("fixture").unsupported.clear();
        let text = render_to_string(&app, 100, 20);
        assert!(text.contains("no associated hosts reported"), "{text}");
        assert!(!text.contains("not exposed by this firmware"), "{text}");
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
            sms_error: Some("request rejected by the firmware (100005)".into()),
            ..Default::default()
        });
        let text = render_to_string(&app, 100, 26);
        assert!(
            text.contains("blocked: request rejected by the firmware (100005)"),
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
        assert!(text.contains("5G NR (bars)"), "\n{text}");
        assert!(text.contains("the firmware sends the nr fields empty"));
        assert!(text.contains("4/5"));
    }

    /// The aux line must wrap to the panel width instead of being clipped by
    /// the non-wrapping paragraph.
    #[test]
    fn nr_aux_lines_wrap_to_width() {
        let nr = crate::model::Nr {
            earfcn: Some("DL:528980 UL:528980".into()),
            dl_bw: Some("90MHz".into()),
            ul_bw: Some("90MHz".into()),
            rank: Some(4),
            bler_pct: Some(0.0),
            cqi: Some(10),
            txpower_dbm: Some(-25.0),
            ..Default::default()
        };
        let wide = crate::ui::tabs::nr_aux_lines(&nr, 200);
        assert_eq!(wide.len(), 1, "one line when it fits");
        let narrow = crate::ui::tabs::nr_aux_lines(&nr, 40);
        assert!(narrow.len() > 1, "wraps when it does not fit");
        assert!(
            narrow.iter().all(|l| l.chars().count() <= 40),
            "no line exceeds the width: {narrow:?}"
        );
        assert_eq!(
            crate::ui::tabs::nr_aux_lines(&crate::model::Nr::default(), 80).len(),
            0,
            "nothing to show when the firmware sent no aux field"
        );
        assert_eq!(
            crate::ui::tabs::nr_aux_lines_head(&crate::model::Nr::default(), 80, "still reports")
                .len(),
            0,
            "a lead-in with nothing after it must not draw a bare word"
        );
    }

    /// The bars panel names the NR fields this firmware does send instead of
    /// dropping them: a unit can report an ARFCN while leaving RSRP empty.
    #[test]
    fn bars_panel_names_the_nr_fields_it_did_get() {
        let mut app = fixture_app();
        app.view = View::Gauges;
        if let Some(s) = app.snap.as_mut() {
            s.connection.endc = true;
            s.connection.nr_bars = 4;
            s.signal.nr = Some(crate::model::Nr {
                earfcn: Some("DL:528980 UL:528980".into()),
                dl_bw: Some("100MHz".into()),
                rank: Some(4),
                ..Default::default()
            });
        }
        let text = render_to_string(&app, 140, 34);
        assert!(text.contains("still reports"), "\n{text}");
        assert!(text.contains("ARFCN DL:528980 UL:528980"), "\n{text}");
        assert!(text.contains("DL 100MHz"), "\n{text}");
        assert!(text.contains("rank 4"), "\n{text}");
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
            text.contains("= device, nothing to apply"),
            "the ticked row must spell the merge and its diff\n{text}"
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
    fn bands_tab_names_the_serving_band_and_the_nr_band() {
        // fixtures/sample-snapshot.json reports band = "B1" and signal.nr = null.
        let mut app = fixture_app();
        app.select_tab(Tab::Bands);
        let text = render_to_string(&app, 100, 30);
        assert!(text.contains("serving"), "no serving label\n{text}");
        assert!(text.contains("B1 LTE"), "serving LTE band missing\n{text}");
        assert!(
            !text.contains("N41"),
            "this firmware reports no NR band, so none may be invented\n{text}"
        );

        // fixtures/sample-snapshot-5g.json is the one that carries NR.
        let snap: crate::model::Snapshot =
            serde_json::from_str(include_str!("../../fixtures/sample-snapshot-5g.json"))
                .expect("5g fixture");
        let mut app5 = fixture_app();
        app5.on_snapshot(snap, 0);
        app5.select_tab(Tab::Bands);
        let text5 = render_to_string(&app5, 100, 30);
        assert!(
            text5.contains("N41 NR"),
            "the NR band the firmware reports must be named\n{text5}"
        );
    }

    #[test]
    fn bands_tab_says_nothing_to_apply_when_the_tick_matches_the_device() {
        let mut app = fixture_app();
        app.select_tab(Tab::Bands);
        let text = render_to_string(&app, 100, 30);
        assert!(
            text.contains("nothing to apply"),
            "an unchanged tick must say so\n{text}"
        );
        assert!(
            text.contains("re-apply"),
            "the re-attach case must stay reachable\n{text}"
        );
    }

    #[test]
    fn bands_tab_names_the_dropped_band_in_the_tick_row() {
        let mut app = fixture_app();
        app.select_tab(Tab::Bands);
        app.bands_selected.retain(|b| *b != 5);
        let text = render_to_string(&app, 100, 30);
        assert!(text.contains("-B5"), "the drop is not named\n{text}");
        assert!(
            text.contains("1 change to apply"),
            "the count is missing\n{text}"
        );
    }

    #[test]
    fn bands_tab_warns_when_a_whole_class_is_dropped() {
        let mut app = fixture_app();
        app.select_tab(Tab::Bands);
        app.bands_selected = vec![1, 3, 32, 34, 39, 7, 38, 40, 41, 42, 43];
        let text = render_to_string(&app, 100, 30);
        assert!(
            text.contains("no low band ticked"),
            "silent class loss\n{text}"
        );
        assert!(
            text.contains("B5"),
            "the warning must name the bands\n{text}"
        );
    }

    #[test]
    fn bands_tab_never_drops_a_band_it_cannot_classify() {
        let mut app = fixture_app();
        app.select_tab(Tab::Bands);
        // B66 has a class (mid), B99 has none: both must be drawn, and the
        // denominator must count every band the device listed.
        app.bands_supported = vec![1, 3, 66, 99];
        app.bands_selected = vec![1];
        let text = render_to_string(&app, 100, 30);
        assert!(
            text.contains("B66") && text.contains("B99"),
            "a band must never fall off the grid\n{text}"
        );
        assert!(
            text.contains("other"),
            "the unknown class needs a row\n{text}"
        );
        assert!(
            text.contains("1 of 4"),
            "the denominator must count every band\n{text}"
        );
    }

    #[test]
    fn bands_presets_are_numbered_and_keep_the_missing_mark() {
        let mut app = fixture_app();
        app.select_tab(Tab::Bands);
        app.band_presets = vec![
            crate::config::BandPreset::new(&[1, 3]),
            crate::config::BandPreset::new(&[1, 3, 66]),
        ];
        app.bands_supported = vec![1, 3, 5, 20];
        let text = render_to_string(&app, 100, 30);
        assert!(text.contains("1:B1+B3"), "presets are not numbered\n{text}");
        assert!(
            text.contains("2:B1+B3+B66!"),
            "the missing mark moved\n{text}"
        );
    }

    #[test]
    fn bands_panel_keeps_its_deciding_rows_at_sixty_columns() {
        let mut app = fixture_app();
        app.select_tab(Tab::Bands);
        let text = render_to_string(&app, 60, 20);
        // The body is 17 rows here, 15 inside the border. The rows that decide
        // must survive; the ones that explain may not.
        for needle in [
            "now",
            "device now",
            "ticked",
            "low",
            "mid",
            "high",
            "10-30 s offline",
        ] {
            assert!(text.contains(needle), "{needle:?} lost at 60x20\n{text}");
        }
        assert!(
            !text.contains("! = band"),
            "the legend cannot fit at 60x20; if this fails the layout grew\n{text}"
        );
    }

    #[test]
    fn bands_tab_warns_when_the_tick_drops_the_serving_band() {
        let mut app = fixture_app();
        app.select_tab(Tab::Bands);
        let calm = render_to_string(&app, 100, 30);
        assert!(
            !calm.contains("not in the tick set"),
            "no warning while B1 is ticked\n{calm}"
        );
        app.bands_selected.retain(|b| *b != 1);
        let text = render_to_string(&app, 100, 30);
        assert!(
            text.contains("B1 carries the link"),
            "the drop must be named before it costs the link\n{text}"
        );
    }

    #[test]
    fn the_gap_warning_needs_a_real_selection_to_compare() {
        // Before the first snapshot lands, the tick list is empty and the device
        // mask is unknown. Warning there would fire on every start-up.
        let mut app = fixture_app();
        app.select_tab(Tab::Bands);
        app.bands_selected.clear();
        let empty = render_to_string(&app, 100, 30);
        assert!(
            !empty.contains("not in the tick set"),
            "an empty tick list is not a decision\n{empty}"
        );
        if let Some(snap) = app.snap.as_mut() {
            snap.net_mode.lte_band = None;
        }
        let unknown = render_to_string(&app, 100, 30);
        assert!(
            !unknown.contains("not in the tick set"),
            "an unknown device mask cannot be compared\n{unknown}"
        );
    }

    #[test]
    fn bands_tab_shows_the_undo_line_only_after_an_apply() {
        let mut app = fixture_app();
        app.select_tab(Tab::Bands);
        let before = render_to_string(&app, 100, 30);
        assert!(
            !before.contains("back to 0x"),
            "nothing to undo yet\n{before}"
        );

        app.undo_mask = Some(vec![1, 3, 5, 8, 20, 28, 32]);
        let after = render_to_string(&app, 100, 30);
        assert!(after.contains("undo"), "{after}");
        assert!(after.contains("· u"), "the key must be named\n{after}");
    }

    #[test]
    fn bands_tab_names_the_link_details_from_the_snapshot() {
        let mut app = fixture_app();
        app.select_tab(Tab::Bands);
        let text = render_to_string(&app, 100, 30);
        assert!(text.contains("link"), "no link row\n{text}");
        assert!(text.contains("2120 MHz"), "dl_mhz missing\n{text}");
        assert!(text.contains("20MHz"), "bandwidth missing\n{text}");
        assert!(text.contains("TM[4]"), "transmission mode missing\n{text}");
        assert!(text.contains("PCI 481"), "pci missing\n{text}");
        assert!(
            text.contains("EARFCN"),
            "earfcn missing on a wide panel\n{text}"
        );

        // Narrower panel: EARFCN is the first thing to go, and the row still fits.
        let narrow = render_to_string(&app, 80, 24);
        assert!(
            narrow.contains("PCI 481"),
            "the row must survive at 80\n{narrow}"
        );
    }

    #[test]
    fn bands_tab_names_the_secondary_carrier() {
        // fixtures/sample-snapshot.json carries dl_mcs_secondary = 9.
        let mut app = fixture_app();
        app.select_tab(Tab::Bands);
        let text = render_to_string(&app, 100, 30);
        assert!(text.contains("secondary DL carrier"), "{text}");
        assert!(text.contains("MCS 9"), "{text}");

        // No second carrier reported: the row must not invent one.
        let mut quiet = fixture_app();
        if let Some(snap) = quiet.snap.as_mut() {
            snap.signal.dl_mcs_secondary = None;
        }
        quiet.select_tab(Tab::Bands);
        let quiet_text = render_to_string(&quiet, 100, 30);
        assert!(!quiet_text.contains("secondary DL carrier"), "{quiet_text}");
    }

    #[test]
    fn bands_tab_names_the_last_apply_and_the_nr_band() {
        let mut app = fixture_app();
        app.select_tab(Tab::Bands);
        app.last_apply = Some(crate::app::LastApply {
            bands: vec![1, 3, 20],
            mode: "03".into(),
            verdict: "verified · router 100006".into(),
            at: std::time::Instant::now(),
        });
        let text = render_to_string(&app, 100, 30);
        assert!(text.contains("last"), "{text}");
        assert!(
            text.contains("0x80005"),
            "the applied mask must be named\n{text}"
        );
        assert!(
            text.contains("4G only"),
            "the mode is spelled, not a raw code\n{text}"
        );
        assert!(text.contains("router 100006"), "{text}");

        // NR comes only from the 5g fixture.
        let snap: crate::model::Snapshot =
            serde_json::from_str(include_str!("../../fixtures/sample-snapshot-5g.json"))
                .expect("5g");
        let mut app5 = fixture_app();
        app5.on_snapshot(snap, 0);
        app5.select_tab(Tab::Bands);
        let text5 = render_to_string(&app5, 100, 30);
        assert!(text5.contains("N41 serving"), "{text5}");

        // A firmware with no NR band: say so instead of inventing a list.
        let mut plain = fixture_app();
        plain.select_tab(Tab::Bands);
        let quiet = render_to_string(&plain, 100, 30);
        assert!(quiet.contains("no NR band reported"), "{quiet}");
    }

    #[test]
    fn the_fill_rows_stay_off_a_short_panel() {
        let mut app = fixture_app();
        app.select_tab(Tab::Bands);
        app.undo_mask = Some(vec![1, 3]);
        app.last_apply = Some(crate::app::LastApply {
            bands: vec![1, 3],
            mode: "03".into(),
            verdict: "verified".into(),
            at: std::time::Instant::now(),
        });
        // 60x20: body 17 rows.
        let tiny = render_to_string(&app, 60, 20);
        for gone in [
            "link  ",
            "secondary DL carrier",
            "no NR band reported",
            "back to 0x",
            "last  ",
        ] {
            assert!(
                !tiny.contains(gone),
                "{gone:?} must not crowd 60x20\n{tiny}"
            );
        }
        // 80x24: body 21 rows, so `link` (20) shows and `ca` (22) does not.
        let mid = render_to_string(&app, 80, 24);
        assert!(mid.contains("link"), "link shows at 80x24\n{mid}");
        assert!(
            !mid.contains("secondary DL carrier"),
            "ca needs 22 body rows\n{mid}"
        );
        assert!(
            !mid.contains("no NR band reported"),
            "nr needs 24 body rows\n{mid}"
        );
    }

    #[test]
    fn a_narrow_panel_truncates_the_long_rows_instead_of_wrapping() {
        // 60x30: narrow but tall, the only shape where the long rows survive and
        // the width is the binding constraint.
        let mut app = fixture_app();
        app.select_tab(Tab::Bands);
        app.bands_selected.retain(|b| *b != 1);
        app.last_apply = Some(crate::app::LastApply {
            bands: vec![1, 3, 20],
            mode: "03".into(),
            verdict: "verified · router 100006".into(),
            at: std::time::Instant::now(),
        });
        let text = render_to_string(&app, 60, 30);
        assert!(text.contains("B1 carries the link"), "{text}");
        let last_line = text
            .lines()
            .find(|l| l.contains("last"))
            .expect("the last row is on screen");
        assert!(
            last_line.contains('…'),
            "a long row is cut, not wrapped: {last_line}"
        );
        assert!(
            text.contains("10-30 s offline"),
            "the action row survives\n{text}"
        );
    }

    #[test]
    fn status_line_carries_the_current_tabs_own_keys() {
        let mut app = fixture_app();
        let cases = [
            (Tab::Bands, "s save"),
            (Tab::Traffic, "d edit DNS"),
            (Tab::Network, "p ping host"),
            (Tab::Clients, "c copy mac"),
            (Tab::Dns, "Enter apply"),
            (Tab::Device, "x dump json"),
            (Tab::Signal, "v view"),
            (Tab::Traffic, "b reset budget"),
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

    fn help_frame(w: u16, h: u16, allow_write: bool) -> String {
        let mut app = fixture_app();
        app.allow_write = allow_write;
        app.help = true;
        render_to_string(&app, w, h)
    }

    #[test]
    fn help_uses_the_room_a_wide_terminal_has() {
        // Before this, the panel was pinned at 52 columns and cut 12 of 21 rows
        // mid-word even on a 120 column terminal.
        let text = help_frame(120, 30, false);
        for needle in [
            "not through the router",
            "back in the tick list",
            "switch section from any view",
            "v / V",
            "S / ,",
        ] {
            assert!(text.contains(needle), "help lost {needle:?}\n{text}");
        }
    }

    #[test]
    fn help_falls_back_to_the_short_rows_and_keeps_them_whole() {
        // 60x20: the rows that survive the cut are the global ones, and the
        // three keys the whole change is about must be among them.
        let text = help_frame(60, 20, false);
        for needle in [
            "theme: next",
            "ping a host",
            "pause / resume polling",
            "move the selection",
        ] {
            assert!(text.contains(needle), "short row cut: {needle:?}\n{text}");
        }
    }

    #[test]
    fn help_keeps_quit_and_the_help_key_on_a_short_screen() {
        // 60x20 has room for 17 body rows; the last two rows are pinned on
        // purpose, and the cut is announced instead of vanishing.
        let text = help_frame(60, 20, false);
        assert!(text.contains("quit"), "the q row must survive\n{text}");
        assert!(text.contains("this help"), "the ? row must survive\n{text}");
        assert!(
            text.contains("more rows"),
            "a cut must be announced\n{text}"
        );
    }

    #[test]
    fn help_never_draws_a_key_into_its_description() {
        // `8 / j k / Enter` used to be 14 characters in a 12 column field and
        // ran straight into its text.
        let text = help_frame(120, 30, false);
        let line = text
            .lines()
            .find(|l| l.contains("j k / ↑ ↓"))
            .expect("the move row");
        let at = line.find("j k / ↑ ↓").expect("keys") + "j k / ↑ ↓".len();
        let after = line[at..].chars().next().expect("padding");
        assert_eq!(after, ' ', "the key column collided: {line}");
    }

    #[test]
    fn help_says_what_this_build_can_do() {
        let read_only = help_frame(120, 30, false);
        assert!(read_only.contains("read-only build"), "{read_only}");
        let writable = help_frame(120, 30, true);
        assert!(writable.contains("writes enabled"), "{writable}");
        assert!(
            !writable.contains("read-only build"),
            "a writable build must not claim read-only\n{writable}"
        );
    }

    #[test]
    fn the_header_keeps_the_state_at_twenty_nine_columns() {
        let mut app = fixture_app();
        app.session = crate::app::SessionState::Live;
        let text = render_to_string(&app, 29, 6);
        assert!(
            text.contains("live"),
            "the state fits but was hidden at 29 columns:\n{text}"
        );
    }

    #[test]
    fn the_help_names_all_nine_sections() {
        let mut app = fixture_app();
        app.help = true;
        let text = render_to_string(&app, 100, 40);
        assert!(text.contains("1..9 / Tab"), "help is stale:\n{text}");
    }

    #[test]
    fn wide_characters_never_widen_the_frame() {
        use unicode_width::UnicodeWidthStr;
        // A wide grapheme is two cells but one char; cutting by chars let
        // `Net📺📺📺📺📺` overflow the room it was given.
        let cut = truncate("Net📺📺📺📺📺", 10);
        assert!(
            UnicodeWidthStr::width(cut.as_str()) <= 10,
            "truncate kept {cut:?} at {} cells",
            UnicodeWidthStr::width(cut.as_str())
        );
        let mut app = fixture_app();
        if let Some(snap) = app.snap.as_mut() {
            snap.plmn.name = "Net📺📺📺📺📺".into();
        }
        // The header must still be the frame width, not wider.
        let text = render_to_string(&app, 80, 20);
        assert_eq!(text.lines().count(), 20);
    }

    #[test]
    fn a_painted_theme_fills_the_settings_modal_too() {
        let mut app = fixture_app();
        app.theme = crate::theme::by_name("paper");
        app.settings_open = true;
        let mut terminal = Terminal::new(TestBackend::new(80, 24)).unwrap();
        terminal.draw(|f| draw(f, &app)).unwrap();
        let buf = terminal.backend().buffer();
        let reset_cells = (0..24u16)
            .flat_map(|y| (0..80u16).map(move |x| (x, y)))
            .filter(|&(x, y)| buf[(x, y)].bg == ratatui::style::Color::Reset)
            .count();
        assert_eq!(
            reset_cells, 0,
            "a modal cell fell back to the terminal background"
        );
    }

    /// The failure popup used to cut its own reason at 66 columns. Every word of a
    /// realistic mismatch message must reach the screen, at two window widths.
    #[test]
    fn a_failed_write_shows_its_whole_reason() {
        for width in [80u16, 40u16] {
            let mut app = fixture_app();
            let err = "mismatch after write: wanted 1.1.1.1 / 1.0.0.1, device keeps 192.168.8.1 / 192.168.8.1";
            app.write = crate::write::WriteState::Failed {
                intent: crate::write::WriteIntent::SetDns {
                    primary: "1.1.1.1".into(),
                    secondary: "1.0.0.1".into(),
                },
                err: err.into(),
            };
            let text = render_to_string(&app, width, 24);
            for word in [
                "mismatch",
                "wanted",
                "1.1.1.1",
                "1.0.0.1",
                "device",
                "keeps",
                "192.168.8.1",
                "dismiss",
            ] {
                assert!(
                    text.contains(word),
                    "{word} was cut away at width {width}\n{text}"
                );
            }
        }
    }

    #[test]
    fn every_signal_series_has_a_row_at_eighteen_lines() {
        let mut app = fixture_app();
        app.tab = Tab::Signal;
        let text = render_to_string(&app, 120, 18);
        for label in ["RSRP", "RSRQ", "SINR"] {
            assert!(text.contains(label), "{label} missing at 120x18\n{text}");
        }
    }

    #[test]
    fn a_short_signal_panel_keeps_every_label() {
        let mut app = fixture_app();
        app.tab = Tab::Signal;
        let text = render_to_string(&app, 120, 15);
        for label in ["RSRP", "RSRQ", "SINR"] {
            assert!(text.contains(label), "{label} missing at 120x15\n{text}");
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
        // The active-tab cell keeps its own accent background - the fill must
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

    /// A DNS panel that draws a writable setting a firmware does not have is the
    /// bug that produced the mismatch popup: the row must say so, in the same words
    /// the refusal uses, and must not appear on a router that has the setting.
    #[test]
    fn the_dns_tab_names_a_firmware_without_the_dns_row() {
        let mut app = fixture_app();
        app.tab = Tab::Dns;
        let before = render_to_string(&app, 110, 40);
        assert!(
            !before.contains("no LAN DHCP DNS setting"),
            "an unknown capability must not be reported as missing: {before}"
        );
        match app.snap.as_mut().and_then(|s| s.dns.as_mut()) {
            Some(d) => d.editable = Some(false),
            None => panic!("the fixture snapshot must carry dns"),
        }
        let frame = render_to_string(&app, 110, 40);
        assert!(
            frame.contains("no: this firmware has no LAN DHCP DNS setting"),
            "{frame}"
        );
    }

    /// The refusal reason is longer than the DNS panel below 140 columns. It used
    /// to run into the right border and lose its tail silently, so the box must
    /// cut the row itself and show the cut. Measured, not guessed: the row is
    /// shorter than the panel interior at 60, 80 and 100, and whole at 140.
    #[test]
    fn the_dns_write_row_is_cut_inside_the_panel() {
        const REASON: &str = "this firmware has no LAN DHCP DNS setting \
            (dhcpsdns_display=0), so the router would ignore it: nothing was sent";
        let mut app = fixture_app();
        app.tab = Tab::Dns;
        app.write = crate::write::WriteState::Failed {
            intent: crate::write::WriteIntent::SetDns {
                primary: "1.1.1.1".into(),
                secondary: "1.0.0.1".into(),
            },
            err: REASON.into(),
        };
        for width in [60u16, 80, 100] {
            let frame = render_to_string(&app, width, 30);
            let row = frame
                .lines()
                .find(|l| l.contains("failed ·"))
                .unwrap_or_else(|| panic!("no write row at {width}\n{frame}"));
            assert!(
                row.contains('…'),
                "at {width} the row must end in a cut mark, not at the border: {row:?}"
            );
            assert!(
                !row.contains("nothing was sent"),
                "the whole reason cannot fit at {width}: {row:?}"
            );
            assert!(
                row.trim_end().ends_with('│'),
                "the row must stay inside its own frame at {width}: {row:?}"
            );
        }
        let wide = render_to_string(&app, 140, 30);
        let row = wide
            .lines()
            .find(|l| l.contains("failed ·"))
            .expect("wide write row");
        assert!(row.contains("nothing was sent"), "{row}");
        assert!(!row.contains('…'), "nothing to cut at 140: {row}");
    }

    /// The Speed section is a trend, not two dials: the header line and a plot
    /// with real braille cells in it. Sliced at the next panel border so the
    /// Signal graphs below (also braille) cannot pass the test for it.
    #[test]
    fn the_speed_section_is_a_mirrored_plot_not_two_dials() {
        let mut app = fixture_app();
        app.tab = Tab::Signal;
        for v in 1..=40 {
            app.history_dl.push_back(v as f64 * 1_000_000.0);
            app.history_ul.push_back(v as f64 * 120_000.0);
        }
        let text = render_to_string(&app, 120, 34);
        let lines: Vec<&str> = text.lines().collect();
        let start = lines
            .iter()
            .position(|l| l.contains("SPEED"))
            .expect("the SPEED panel is drawn");
        let end = lines[start + 1..]
            .iter()
            .position(|l| l.trim_start().starts_with('┌'))
            .map(|i| start + 1 + i)
            .unwrap_or(lines.len());
        let panel = lines[start..end].join("\n");
        assert!(panel.contains("peak") || panel.contains("▲"), "{panel}");
        let braille = panel
            .chars()
            .filter(|c| ('\u{2800}'..='\u{28ff}').contains(c))
            .count();
        assert!(
            braille > 40,
            "the plot drew {braille} braille cells in {} rows\n{panel}",
            end - start
        );
    }

    #[test]
    fn a_speed_panel_with_no_samples_says_so() {
        let mut app = fixture_app();
        app.tab = Tab::Signal;
        app.history_dl.clear();
        app.history_ul.clear();
        let text = render_to_string(&app, 120, 34);
        assert!(text.contains("waiting for the first sample"));
    }

    #[test]
    fn the_traffic_tab_shows_the_usage_table_when_tall() {
        let mut app = fixture_app();
        app.tab = Tab::Traffic;
        let text = render_to_string(&app, 120, 34);
        assert!(text.contains("USAGE"), "{text}");
        assert!(text.contains("today"));
        assert!(text.contains("week"));
        assert!(text.contains("month"));
    }

    #[test]
    fn a_short_traffic_tab_names_the_missing_table_instead_of_clipping() {
        let mut app = fixture_app();
        app.tab = Tab::Traffic;
        let text = render_to_string(&app, 80, 24);
        assert!(text.contains("usage table needs 24 rows"), "{text}");
        for line in text.lines() {
            assert!(line.chars().count() <= 80, "a line overflowed: {line:?}");
        }
    }

    #[test]
    fn the_budget_modal_answers_its_question() {
        let mut app = fixture_app();
        app.tab = Tab::Traffic;
        app.reset_confirm = true;
        let text = render_to_string(&app, 100, 30);
        assert!(text.contains("reset the traffic budget?"), "{text}");
        assert!(text.contains("not touched"));
    }

    #[test]
    fn the_traffic_status_line_names_the_reset_key() {
        let mut app = fixture_app();
        app.select_tab(Tab::Traffic);
        let text = render_to_string(&app, 160, 30);
        let status = text.lines().last().unwrap_or_default();
        assert!(status.contains("b reset budget"), "{status}");
        assert!(
            status.contains("q quit"),
            "global keys must survive: {status}"
        );
    }
}
