//! Frame composition: header, tab bar, body, status line, help overlay.

pub mod chart;
pub mod dash;
#[cfg(test)]
mod dash_tests;
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
        draw_help(f, area, app);
    }
    if app.events_open {
        draw_events(f, area, app);
    }

    if !matches!(app.write, crate::write::WriteState::Idle) {
        draw_write_modal(f, area, app);
    }
    if let Some(plan) = &app.sms.confirm {
        draw_sms_modal(f, area, app, plan);
    }
    // Before settings, so an open settings overlay still wins.
    if app.reset_confirm {
        draw_reset_modal(f, area, app);
    }
    if app.settings_open {
        draw_settings(f, area, app);
    }
    if app.speed_confirm {
        draw_speed_modal(f, area, app);
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
        ("graph", app.graph.name().to_string(), "config only"),
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

/// The speed test spends data, so it asks first and names what it will spend.
/// The numbers come from the same clamped config the run reads.
fn draw_speed_modal(f: &mut Frame, area: Rect, app: &App) {
    let t = &app.theme;
    let w = area.width.min(58);
    let h = 7.min(area.height);
    let rect = Rect {
        x: area.x + (area.width - w) / 2,
        y: area.y + (area.height - h) / 2,
        width: w,
        height: h,
    };
    f.render_widget(Clear, rect);
    let cfg = app.speed_cfg.clone().sanitised();
    let spend = if cfg.download_only {
        format!("up to {} MB down, no upload", cfg.max_mb)
    } else {
        format!("up to {} MB down + {} MB up", cfg.max_mb, cfg.upload_mb)
    };
    let lines = vec![
        Line::from(Span::styled(
            "run a speed test?",
            Style::default().fg(t.fg).add_modifier(Modifier::BOLD),
        )),
        Line::from(""),
        Line::from(Span::styled(
            "through this host, not the router",
            Style::default().fg(t.dim),
        )),
        Line::from(Span::styled(spend, Style::default().fg(t.dim))),
        Line::from(Span::styled(
            "counts in the router's month total",
            Style::default().fg(t.dim),
        )),
        Line::from(Span::styled(
            "Enter run   ·   Esc cancel",
            Style::default().fg(t.dim),
        )),
    ];
    f.render_widget(
        Paragraph::new(lines)
            .wrap(Wrap { trim: true })
            .block(tabs::panel("SPEED TEST".into(), t)),
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
                WriteIntent::RenewIp => " renew wan ip? ",
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

/// The SMS confirmation, in the same shape as the band write modal: the plan,
/// what it does to the router, and the two keys that answer it.
fn draw_sms_modal(f: &mut Frame, area: Rect, app: &App, plan: &crate::app::SmsPlan) {
    use crate::write::SmsOp;
    let t = &app.theme;
    let deleting = plan.op == SmsOp::Delete;
    let title = if deleting {
        " delete messages? "
    } else {
        " mark read? "
    };
    let mut body = vec![plan.label(), String::new()];
    for index in plan.indices.iter().take(8) {
        body.push(format!("  index {index}"));
    }
    if plan.indices.len() > 8 {
        body.push(format!("  … and {} more", plan.indices.len() - 8));
    }
    body.push(String::new());
    if deleting {
        body.push("a deleted message cannot be recovered".into());
        body.push("every delete is verified by re-reading the inbox".into());
    } else {
        body.push("the flag is set on the router, then read back".into());
    }
    body.push(String::new());
    body.push("y confirm   ·   n / Esc cancel".into());

    let w = 68.min(area.width);
    let inner_w = w.saturating_sub(2);
    let lines: Vec<Line> = body
        .into_iter()
        .map(|l| Line::from(Span::styled(l, Style::default().fg(t.fg))))
        .collect();
    let para = Paragraph::new(lines).wrap(Wrap { trim: false }).block(
        Block::bordered()
            .title(Span::styled(
                title,
                Style::default().fg(t.warn).add_modifier(Modifier::BOLD),
            ))
            .border_style(Style::default().fg(t.warn)),
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

/// `0.6s` under ten seconds, `41s` above: the header is one row, and the
/// difference between 12.3 and 12 does not earn two more columns.
fn short_age(age: std::time::Duration) -> String {
    let secs = age.as_secs_f32();
    if secs < 10.0 {
        format!("{secs:.1}s")
    } else {
        format!("{}s", age.as_secs())
    }
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
    let mut right_width: u16 = right
        .iter()
        .map(|span| UnicodeWidthStr::width(span.content.as_ref()) as u16)
        .sum();
    // One age per tier: a single number could not say which group was late, so a
    // clients read that took 2.3 s looked like dead data behind a live state.
    let ages = crate::worker::Tier::ALL
        .iter()
        .filter_map(|tier| {
            app.age_for(*tier)
                .map(|age| format!("{} {}", crate::worker::tier_label(*tier), short_age(age)))
        })
        .collect::<Vec<_>>()
        .join(" · ");
    let age_span = if ages.is_empty() {
        format!(" · {age_text}")
    } else {
        format!(" · {ages}")
    };
    let age_w = UnicodeWidthStr::width(age_span.as_str()) as u16;
    if right_width + age_w + 34 <= area.width {
        right.push(Span::styled(age_span, age_style));
        right_width += age_w;
    }
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
    // Nine full titles need ~96 columns; below that, abbreviate instead of clipping.
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
        // The view is a real axis (v cycles it); the renderer name was a second
        // half of a choice that no longer exists, so only the view is named.
        spans.push(Span::styled(
            format!("   {}", app.view.name()),
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
    // Bands: the mask and the write's own progress belong on this row. The
    // panel spells the same keys above, but this row survives when the panel is
    // too short to draw one, and the mask is what `a` is about.
    if app.tab == crate::app::Tab::Bands && area.width >= 80 {
        spans.push(Span::styled(
            format!(" · 0x{}", app.band_mask_hex()),
            Style::default().fg(t.dim),
        ));
        if app.write.is_busy() {
            spans.push(Span::styled(
                format!(" · {}", app.write.label()),
                Style::default().fg(t.warn),
            ));
        }
    }
    // SMS: the marks, the last outcome and a refusal all belong on this row.
    // The list title repeats the count, because a pane can be scrolled away.
    if app.tab == crate::app::Tab::Sms {
        if !app.sms.selected.is_empty() {
            spans.push(Span::styled(
                format!(" · {} selected", app.sms.selected.len()),
                Style::default().fg(t.accent),
            ));
        }
        if app.sms.busy {
            spans.push(Span::styled(
                " · working…".to_string(),
                Style::default().fg(t.warn),
            ));
        }
        if let Some(line) = &app.sms.notice {
            spans.push(Span::styled(
                format!(" · {line}"),
                Style::default().fg(t.accent),
            ));
        }
        if let Some(why) = &app.sms.blocked {
            spans.push(Span::styled(
                format!(" · {why}"),
                Style::default().fg(t.danger),
            ));
        }
    }
    let mut used = spans
        .iter()
        .map(|s| s.content.chars().count())
        .sum::<usize>();
    // The revert offer sits beside the section name: it is a fact about the
    // write that just landed, not a notice that replaces the last one.
    if let Some(line) = app.watch_line() {
        let text = format!(" · {line}");
        used += text.chars().count();
        spans.push(Span::styled(text, Style::default().fg(t.accent)));
    }
    // This section's own keys sit next to the global ones. They are the first
    // thing to give up space on a narrow terminal (never `q quit · ? help`).
    // `i events` is discoverability, not a requirement: it is advertised only
    // when the line can still hold it and still leave room for a hint.
    const GLOBAL_TAIL: &str = " · q quit · ? help";
    // A notice is bounded by the keys behind it, not by half the row: half the
    // row plus a router name and a key list still overflows 80 columns, and then
    // the row ends mid-key.
    let note_room = (area.width as usize)
        .saturating_sub(used + GLOBAL_TAIL.chars().count() + 4)
        .max(8);
    if let Some(note) = app.live_notice() {
        let text = format!(" · {}", truncate(note, note_room));
        used += text.chars().count();
        spans.push(Span::styled(text, Style::default().fg(t.accent)));
    } else if let Some(err) = app.error_log.front() {
        let text = format!(" · {}", truncate(err, note_room));
        used += text.chars().count();
        spans.push(Span::styled(text, Style::default().fg(t.danger)));
    }

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

fn draw_help(f: &mut Frame, area: Rect, app: &App) {
    help::draw(
        f,
        area,
        &app.theme,
        app.allow_write,
        app.help_off,
        &app.help_cap,
    );
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
mod sms_tests;

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
            GraphStyle::Braille,
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
            app.history_rsrp.push_back(-100.0 + (i % 20) as f64);
            app.history_rsrq.push_back(-12.0);
            app.history_sinr.push_back(4.0);
        }
        let frame = crate::ui::render_to_string(&app, 120, 42);
        assert!(
            frame.contains("SIGNAL MAP · 240s"),
            "the panel names the window it draws"
        );
        assert!(
            frame.contains("-240s") && frame.contains("-120s") && frame.contains("now"),
            "the ruler spans the window"
        );
        assert!(
            frame.contains("-100..-81  avg -91  now fair"),
            "the map carries its own statistics: range, average and the state of the newest reading"
        );
        assert!(!frame.contains("120 samples"), "the old claim is gone");
    }

    #[test]
    fn a_short_history_keeps_every_bar_one_cell_wide() {
        let mut app = App::new(
            crate::theme::by_name("dark"),
            GraphStyle::Braille,
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
        let frame = crate::ui::render_to_string(&app, 120, 42);
        let row = frame
            .lines()
            .find(|l| l.contains("↓ down"))
            .expect("throughput down row");
        let ramp: Vec<usize> = row
            .chars()
            .enumerate()
            .filter(|(_, c)| ('\u{2581}'..='\u{2588}').contains(c))
            .map(|(i, _)| i)
            .collect();
        assert!(!ramp.is_empty(), "a sample has to draw: {row:?}");
        assert!(
            ramp.len() <= 12,
            "fifteen samples at 2s each cover ten 3s cells, not the whole plot"
        );
        let cells = row.chars().filter(|c| *c == ' ' || *c == '│').count();
        assert!(
            *ramp.first().unwrap() > cells / 2,
            "a short record sits at the right edge, it is not stretched: {row:?}"
        );
        assert!(
            frame.contains("-240s") && frame.contains("now"),
            "the ruler stays the window, whatever the record holds"
        );
    }

    #[test]
    fn a_single_sample_says_collecting_and_draws_one_mark() {
        let mut app = App::new(
            crate::theme::by_name("dark"),
            GraphStyle::Braille,
            View::Full,
            100,
            2000,
        );
        app.snap = Some(
            serde_json::from_str(include_str!("../../fixtures/sample-snapshot.json")).unwrap(),
        );
        app.history_dl.push_back(4_000_000.0);
        app.history_ul.push_back(200_000.0);
        let frame = crate::ui::render_to_string(&app, 120, 42);
        let row = frame
            .lines()
            .find(|l| l.contains("↓ down"))
            .expect("throughput down row");
        let marks = row
            .chars()
            .filter(|c| ('\u{2581}'..='\u{2588}').contains(c))
            .count();
        assert_eq!(marks, 1, "one reading is one mark: {row:?}");
        assert!(
            !frame.contains("worst "),
            "one sample supports no conclusion line: {frame}"
        );
    }

    fn fixture_app() -> App {
        let snap: crate::model::Snapshot =
            serde_json::from_str(include_str!("../../fixtures/sample-snapshot.json"))
                .expect("fixture snapshot");
        let mut app = App::new(
            Theme::get("terminal"),
            GraphStyle::Braille,
            View::Full,
            100,
            2000,
        );
        app.on_snapshot(snap, 432);
        app
    }

    /// A frame drawn before the first sample names what it is doing instead of
    /// drawing a zero. `snap == None` is the boot frame, and the router has not
    /// said anything yet.
    #[test]
    fn the_boot_frame_shows_a_collecting_marker() {
        let mut app = boot_app();
        app.tab = crate::app::Tab::Clients;
        let frame = render_to_string(&app, 120, 34);
        assert!(
            frame.contains("collecting the first sample"),
            "the boot frame must say it is collecting: {frame}"
        );
    }

    /// Nothing has arrived, so no field may carry a number: a lone dash is the
    /// only honest value in the panel.
    #[test]
    fn an_empty_field_is_a_dash_not_a_fabricated_value() {
        let mut app = boot_app();
        app.tab = crate::app::Tab::Clients;
        let frame = render_to_string(&app, 120, 34);
        let rows: Vec<&str> = frame.lines().collect();
        let top = rows
            .iter()
            .position(|r| r.contains("CLIENTS"))
            .expect("the clients panel is named");
        let body: Vec<&str> = rows[top + 1..]
            .iter()
            .take_while(|r| !r.trim_start().starts_with('└'))
            .copied()
            .collect();
        assert!(
            body.iter().any(|r| r.contains('-')),
            "an unfilled field shows a dash: {frame}"
        );
        assert!(
            body.iter().all(|r| !r.chars().any(|c| c.is_ascii_digit())),
            "no field may carry a number before the first sample: {frame}"
        );
    }

    fn boot_app() -> App {
        App::new(
            Theme::get("terminal"),
            GraphStyle::Braille,
            View::Full,
            100,
            2000,
        )
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
        // that opens with one of the four padded class labels. A `warn` line
        // mentions "high" too and must never enter this check.
        const LEAD: usize = 5;
        const CELL: usize = 5;
        let grid: Vec<String> = text
            .lines()
            .filter(|l| {
                let chars: Vec<char> = l.chars().collect();
                if chars.len() <= LEAD {
                    return false;
                }
                let cls: String = chars[..LEAD].iter().collect();
                ["low  ", "mid  ", "high ", "other"].contains(&cls.as_str())
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
                        i >= LEAD && (i - LEAD).is_multiple_of(CELL),
                        "band token at char {i} is off the {CELL}-column grid: {line}"
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
        // The instrument band is gone from this layout; what a short terminal
        // still owes the reader is every zone, one column, no statistics block.
        for title in ["RADIO", "LINK / SESSION", "SIGNAL MAP", "THROUGHPUT"] {
            assert!(text.contains(title), "{title} disappeared: {text}");
        }
        assert!(
            !text.contains("min "),
            "the statistics column needs a width this size does not have: {text}"
        );
        assert!(
            text.contains("1 cell = ") && text.contains("last 240s"),
            "the cell widens when the plot does not: {text}"
        );
    }

    #[test]
    fn the_header_shows_an_age_per_tier() {
        use crate::worker::Tier;
        let mut app = fixture_app();
        let now = std::time::Instant::now();
        app.stamps.set(Tier::Radio, now);
        app.stamps
            .set(Tier::Clients, now - std::time::Duration::from_secs(12));
        app.stamps
            .set(Tier::Static, now - std::time::Duration::from_secs(41));
        let text = render_to_string(&app, 140, 30);
        let head = text.lines().next().unwrap_or_default();
        assert!(head.contains("signal "), "no radio age: {head}");
        assert!(head.contains("clients 12s"), "no clients age: {head}");
        assert!(head.contains("device 41s"), "no device age: {head}");
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
    fn the_status_line_offers_the_revert_while_the_window_is_open() {
        let mut app = fixture_app();
        app.select_tab(Tab::Bands);
        assert_eq!(
            app.watch_line(),
            None,
            "no window before an apply lands: the row must stay as it was"
        );
        let plain = render_to_string(&app, 120, 34);
        assert!(!plain.contains("u reverts"), "{plain}");

        app.watch_until = Some(std::time::Instant::now() + std::time::Duration::from_secs(180));
        let text = render_to_string(&app, 120, 34);
        assert!(text.contains("watching "), "{text}");
        assert!(text.contains("u reverts"), "{text}");
        let status = text.lines().last().unwrap_or_default();
        assert!(
            status.contains("u reverts"),
            "the offer belongs in the status bar: {status}"
        );
        assert!(
            status.contains("q quit"),
            "keys are not displaced: {status}"
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
        assert!(after.contains("SERVING CELL"), "\n{after}");
    }

    #[test]
    fn bands_tab_shows_merge_presets_and_the_merge_line() {
        let mut app = fixture_app();
        app.select_tab(Tab::Bands);
        let text = render_to_string(&app, 120, 30);
        assert!(text.contains("PRESETS"), "preset region missing\n{text}");
        assert!(text.contains("B1+B3"), "ready-made merge missing\n{text}");
        assert!(
            text.contains("= device · nothing to apply"),
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
        // The panel must use the width it is given: 52 columns cut 12 of 21 rows
        // mid-word even on a 120 column terminal.
        let text = help_frame(120, 30, false);
        for needle in [
            "not through the router",
            "back in the tick list",
            "the ticked merge",
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
        // 60x20 has room for 17 body lines; the window takes 16 of them and the
        // last line names where the reader is, so the cut is announced instead
        // of the rows vanishing.
        let text = help_frame(60, 20, false);
        assert!(text.contains("quit"), "the q row must survive\n{text}");
        assert!(text.contains("this help"), "the ? row must survive\n{text}");
        assert!(
            text.contains("rows 1-15 of 45"),
            "the window must say where it is\n{text}"
        );
    }

    #[test]
    fn help_never_draws_a_key_into_its_description() {
        // The key column must hold `8 / j k / Enter` (14 characters), or the
        // keys run straight into the description.
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

    /// A heading owns the rows under it: two headings in a row, a heading last,
    /// or a key row before any heading all say the list was assembled wrong.
    #[test]
    fn every_help_heading_has_a_row_under_it() {
        assert!(
            matches!(
                crate::ui::help::rows().first(),
                Some(crate::ui::help::HelpRow::Heading(_))
            ),
            "the first row must be a heading, not a key outside any section"
        );
        let mut sections = 0;
        let mut rows_under = 0;
        for row in crate::ui::help::rows() {
            match row {
                crate::ui::help::HelpRow::Heading(t) => {
                    assert!(
                        rows_under > 0 || sections == 0,
                        "the section {t:?} has no row under it"
                    );
                    sections += 1;
                    rows_under = 0;
                }
                crate::ui::help::HelpRow::Key(_) => rows_under += 1,
            }
        }
        assert!(sections > 1, "the list must be grouped");
        assert!(rows_under > 0, "the last section has no row under it");
    }

    /// At a size that shows the whole list, every heading is drawn, in the data's
    /// order, directly above the keys it names.
    #[test]
    fn the_help_groups_the_keys_under_a_heading() {
        let text = help_frame(120, 48, false);
        assert!(
            !text.contains("rows 1-"),
            "48 rows hold all 45 lines, so nothing may be windowed\n{text}"
        );
        let lines: Vec<&str> = text.lines().collect();
        let mut at = 0usize;
        for row in crate::ui::help::rows() {
            if let crate::ui::help::HelpRow::Heading(t) = row {
                let found = lines
                    .iter()
                    .skip(at)
                    .position(|l| l.contains(t))
                    .unwrap_or_else(|| panic!("the section {t:?} is not drawn\n{text}"));
                at += found + 1;
            }
        }
        let net = lines
            .iter()
            .position(|l| l.contains("NETWORK (Tab 4)"))
            .expect("the network section");
        assert!(
            lines[net + 1].contains("download/upload test"),
            "the row under the network heading must be T: {:?}",
            lines[net + 1]
        );
    }

    /// The window scrolls instead of hiding rows: it names the lines it shows,
    /// and a section scrolled past is gone from the frame.
    #[test]
    fn the_help_window_scrolls_and_names_the_rows_it_shows() {
        let mut app = fixture_app();
        app.help = true;
        let top = render_to_string(&app, 60, 20);
        assert!(top.contains("GLOBAL KEYS"), "{top}");
        assert_eq!(
            app.help_cap.get(),
            16,
            "the renderer must tell the keys how many lines fit"
        );
        assert!(
            !top.contains("DNS (Tab 8)"),
            "the last section cannot fit in a 16 line window\n{top}"
        );
        app.help_off = u16::MAX;
        let end = render_to_string(&app, 60, 20);
        assert!(end.contains("DNS (Tab 8)"), "{end}");
        assert!(end.contains("rows 30-45 of 45"), "{end}");
        assert!(!end.contains("GLOBAL KEYS"), "{end}");
    }

    /// One frame shows one window of the list, so this reads the data: the
    /// sections are named once each and none is blank. What the frame shows of
    /// them is the grouping test's job.
    #[test]
    fn the_help_names_every_section_it_has() {
        let mut app = fixture_app();
        app.help = true;
        let text = render_to_string(&app, 100, 40);
        assert!(text.contains("1..9 / Tab"), "help is stale:\n{text}");
        let mut seen: Vec<&'static str> = Vec::new();
        for row in crate::ui::help::rows() {
            if let crate::ui::help::HelpRow::Heading(t) = row {
                assert!(!t.is_empty(), "a section must be named");
                assert!(!seen.contains(&t), "the section {t:?} is named twice");
                seen.push(t);
            }
        }
        assert!(seen.len() > 1, "the list must be grouped: {seen:?}");
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

    /// The failure popup shows its whole reason: every word of a realistic
    /// mismatch message must reach the screen, at two window widths.
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

    #[test]
    fn a_finished_test_is_drawn_with_its_radio_and_its_window() {
        let mut app = fixture_app();
        app.select_tab(Tab::Network);
        app.speed = Some(crate::speedtest::SpeedResult {
            download_bps: 94_000_000.0,
            upload_bps: 12_000_000.0,
            ttfb_ms: 18.0,
            down_bytes: 50_000_000,
            up_bytes: 10_000_000,
            down_secs: 4.2,
            up_secs: 6.6,
            download_url: "http://x/".into(),
            upload_url: "http://y/".into(),
            partial: false,
            short_window: false,
            note: None,
            at_unix: crate::app::unix_now(),
        });
        app.speed_context = Some(crate::app::SpeedContext {
            net_type: "LTE".into(),
            band_at_start: Some("LTE B3".into()),
            band: "LTE B3".into(),
            rsrp_dbm: Some(-92.0),
            rsrq_db: Some(-9.0),
            sinr_db: Some(11.0),
            // The device counter is bytes per second: 11.5 MB/s is 92 Mbps and
            // must be rendered through `bps`, never through `bitrate`.
            router_peak_dl: 11_500_000,
            router_peak_ul: 2_000_000,
        });
        let text = render_to_string(&app, 120, 40);
        assert!(text.contains("94.00 Mbps"), "{text}");
        assert!(
            text.contains("this host"),
            "the direction must be on screen\n{text}"
        );
        assert!(text.contains("LTE B3"), "{text}");
        assert!(
            text.contains("4.2 s"),
            "the window must be visible, not implied\n{text}"
        );
        assert!(
            text.contains("peak device ↓ 92.00 Mbps"),
            "the device peak is bytes and must go through bps\n{text}"
        );
    }

    #[test]
    fn a_short_window_is_labelled_ramp_limited() {
        let mut app = fixture_app();
        app.select_tab(Tab::Network);
        app.speed = Some(crate::speedtest::SpeedResult {
            download_bps: 400_000_000.0,
            down_secs: 0.9,
            short_window: true,
            ..crate::speedtest::SpeedResult::default()
        });
        let text = render_to_string(&app, 120, 40);
        assert!(text.contains("ramp-limited"), "{text}");
    }

    #[test]
    fn a_running_test_shows_progress_not_a_stale_result() {
        let mut app = fixture_app();
        app.select_tab(Tab::Network);
        let (run, _, _) = app.start_speed_test();
        app.on_speed_progress(
            run,
            crate::speedtest::SpeedProgress {
                phase: crate::speedtest::Phase::Upload,
                bytes: 4_000_000,
                rate_bps: 24_000_000.0,
            },
        );
        let text = render_to_string(&app, 120, 40);
        assert!(text.contains("uploading"), "{text}");
        assert!(text.contains("24.00 Mbps"), "{text}");
    }

    /// `network_tab` returns early when `snap` is None; the speed panel must be
    /// drawn before that guard, or a finished test disappears with the router.
    #[test]
    fn the_speed_panel_renders_before_the_first_snapshot() {
        let mut app = App::new(
            crate::theme::Theme::get("terminal"),
            GraphStyle::Braille,
            View::Full,
            100,
            2000,
        );
        app.select_tab(Tab::Network);
        app.speed = Some(crate::speedtest::SpeedResult {
            download_bps: 20_000_000.0,
            ..crate::speedtest::SpeedResult::default()
        });
        let text = render_to_string(&app, 120, 40);
        assert!(text.contains("20.00 Mbps"), "{text}");
    }

    #[test]
    fn the_panel_survives_eighty_by_twenty_four() {
        let mut app = fixture_app();
        app.select_tab(Tab::Network);
        app.speed = Some(crate::speedtest::SpeedResult {
            download_bps: 94_000_000.0,
            ..crate::speedtest::SpeedResult::default()
        });
        let text = render_to_string(&app, 80, 24);
        assert!(text.contains("PING"), "{text}");
        assert!(text.contains("94.00 Mbps"), "{text}");
    }

    #[test]
    fn the_dashboard_carries_one_test_line() {
        let mut app = fixture_app();
        app.speed = Some(crate::speedtest::SpeedResult {
            download_bps: 94_000_000.0,
            upload_bps: 12_000_000.0,
            down_secs: 4.2,
            at_unix: crate::app::unix_now(),
            ..crate::speedtest::SpeedResult::default()
        });
        let text = render_to_string(&app, 130, 40);
        assert!(text.contains("test"), "{text}");
        assert!(text.contains("94.00 Mbps"), "{text}");
    }

    #[test]
    fn the_help_lists_the_speed_key() {
        let rows = crate::ui::help::key_rows();
        assert!(
            rows.iter().any(|r| r.keys == "T"),
            "the help must name the key T opens"
        );
    }

    /// The Device tab advertises `W renew` in the status bar and the help only
    /// if the handler really answers to it, filed under the section whose key it
    /// is: the heading carries the section, not the row's own text.
    #[test]
    fn the_renew_row_is_in_the_help_and_the_device_hint() {
        let mut section: &'static str = "";
        let mut found = None;
        for row in crate::ui::help::rows() {
            match row {
                crate::ui::help::HelpRow::Heading(t) => section = t,
                crate::ui::help::HelpRow::Key(k) if k.keys == "W" => found = Some((section, k)),
                _ => {}
            }
        }
        let (section, row) = found.expect("the help must name the key W opens");
        assert!(row.short.contains("renew"), "{}", row.short);
        assert_eq!(
            section, "DEVICE (Tab 7)",
            "the row must sit under the section its key belongs to"
        );
        let hint = Tab::Device.hints();
        assert!(hint.contains("W renew"), "{hint}");
    }

    #[test]
    fn age_label_counts_from_the_result_time() {
        assert_eq!(crate::ui::tabs::age_label(1_000, 1_030), "30s ago");
        assert_eq!(crate::ui::tabs::age_label(1_000, 1_600), "10m ago");
        assert_eq!(crate::ui::tabs::age_label(1_000, 7_400), "1h ago");
    }

    #[test]
    fn bands_layout_draws_the_seven_regions() {
        let mut app = fixture_app();
        app.select_tab(Tab::Bands);
        for (w, h) in [(120, 38), (200, 50), (130, 30)] {
            let text = render_to_string(&app, w, h);
            for needle in [
                "1 SERVING CELL",
                "2 BAND MATRIX",
                "3 MASK BITS",
                "4 APPLY",
                "5 PRESETS",
                "6 WRITE LOG",
            ] {
                assert!(
                    text.contains(needle),
                    "{needle:?} missing at {w}x{h}\n{text}"
                );
            }
            let status = text.lines().last().unwrap_or_default();
            assert!(status.contains("q quit"), "status row lost at {w}x{h}");
        }
    }

    #[test]
    fn bands_layout_stacks_below_a_hundred_columns() {
        let mut app = fixture_app();
        app.select_tab(Tab::Bands);
        let text = render_to_string(&app, 80, 24);
        for needle in ["1 SERVING CELL", "2 BAND MATRIX", "4 APPLY", "6 WRITE LOG"] {
            assert!(text.contains(needle), "{needle:?} missing at 80x24\n{text}");
        }
        // The stacked tier draws plain blocks: a border would eat two of four rows.
        assert!(
            !text.contains("┌ 1 SERVING CELL"),
            "the stacked tier must not spend rows on borders\n{text}"
        );
    }

    #[test]
    fn bands_layout_never_paints_outside_the_panel() {
        let mut app = fixture_app();
        app.select_tab(Tab::Bands);
        for (w, h) in [(80, 24), (60, 20), (40, 12), (24, 10)] {
            let text = render_to_string(&app, w, h);
            for line in text.lines() {
                let n = line.chars().count();
                assert!(
                    n <= w as usize,
                    "a {n}-char row cannot fit {w} columns: {line:?}"
                );
            }
            assert!(!text.is_empty(), "no frame at {w}x{h}");
        }
    }

    #[test]
    fn the_tab_bar_offers_no_choice_that_does_not_exist() {
        let mut app = fixture_app();
        let text = render_to_string(&app, 120, 34);
        let bar = text
            .lines()
            .find(|l| l.contains("Dashboard"))
            .unwrap_or_default()
            .to_string();
        assert!(
            bar.contains(" full"),
            "the bar still names the view, which is a real axis: {bar}"
        );
        for gone in ["dots", "bars", "graph", "braille", "ascii", "full/"] {
            assert!(
                !bar.contains(gone),
                "the tab bar still names the retired renderer via {gone:?}: {bar}"
            );
        }
        // The settings overlay keeps a readings row, not a cycle.
        app.settings_open = true;
        let text = render_to_string(&app, 120, 34);
        assert!(text.contains("braille"), "\n{text}");
        assert!(
            !text.contains("g cycles") && !text.contains("cycle the graph"),
            "no key is offered for a choice that is gone\n{text}"
        );
    }

    /// The buffer itself, so a test can ask what colour a cell was drawn in.
    fn buffer_of(app: &App, width: u16, height: u16) -> ratatui::buffer::Buffer {
        let backend = TestBackend::new(width, height);
        let mut terminal = Terminal::new(backend).expect("test terminal");
        terminal.draw(|f| draw(f, app)).expect("draw");
        terminal.backend().buffer().clone()
    }

    /// The first row whose text contains `needle`.
    fn row_with(buf: &ratatui::buffer::Buffer, width: u16, height: u16, needle: &str) -> u16 {
        (0..height)
            .find(|y| {
                let line: String = (0..width).map(|x| buf[(x, *y)].symbol()).collect();
                line.contains(needle)
            })
            .unwrap_or_else(|| panic!("no row contains {needle:?}"))
    }

    /// Colours of the glyph cells on a row, label included, deduplicated.
    fn row_colours(
        buf: &ratatui::buffer::Buffer,
        y: u16,
        width: u16,
    ) -> Vec<ratatui::style::Color> {
        let mut seen = Vec::new();
        for x in 0..width {
            let cell = &buf[(x, y)];
            if !cell.symbol().trim().is_empty() && !seen.contains(&cell.fg) {
                seen.push(cell.fg);
            }
        }
        seen
    }

    /// Colours of the value part only: everything that is not the dim label.
    fn value_colours(
        buf: &ratatui::buffer::Buffer,
        y: u16,
        width: u16,
        label_colour: ratatui::style::Color,
    ) -> Vec<ratatui::style::Color> {
        let mut seen = Vec::new();
        for x in 0..width {
            let cell = &buf[(x, y)];
            if !cell.symbol().trim().is_empty()
                && cell.fg != label_colour
                && !seen.contains(&cell.fg)
            {
                seen.push(cell.fg);
            }
        }
        seen
    }

    /// A measurement is drawn in one colour, the palette's `value` slot, so a
    /// reader can tell a reading from a label at a glance.
    #[test]
    fn a_neutral_measurement_uses_the_value_colour() {
        let mut app = fixture_app();
        app.select_tab(Tab::Device);
        let buf = buffer_of(&app, 120, 34);
        let y = row_with(&buf, 120, 34, "uptime");
        let colours = value_colours(&buf, y, 120, app.theme.dim);
        assert!(
            !colours.is_empty(),
            "the uptime row carries no value glyphs at all"
        );
        assert_eq!(
            colours,
            vec![app.theme.value],
            "a duration is a measurement and must use the value colour"
        );
    }

    /// A state keeps its own colour: the value slot must never repaint it.
    #[test]
    fn a_semantic_cell_keeps_its_state_colour() {
        let mut app = fixture_app();
        // The CONNECTION panel is drawn by the dense view, the one place a
        // connection state shares a frame with the measurements.
        app.view = View::Dense;
        let buf = buffer_of(&app, 120, 34);
        let y = row_with(&buf, 120, 34, "status");
        let colours = value_colours(&buf, y, 120, app.theme.dim);
        assert_eq!(
            colours,
            vec![app.theme.ok],
            "connection state is semantic, so it keeps `ok`: {colours:?}"
        );
        assert_ne!(
            app.theme.value, app.theme.ok,
            "the palette would collapse the distinction this test relies on"
        );
    }

    /// An identifier is one indivisible fact: `B1` or an IMEI must not arrive
    /// in two colours, so it keeps the body colour and is never split.
    #[test]
    fn an_identifier_is_never_split_into_two_colours() {
        let mut app = fixture_app();
        app.select_tab(Tab::Device);
        let buf = buffer_of(&app, 120, 34);
        for label in ["imei", "serial"] {
            let y = row_with(&buf, 120, 34, label);
            let colours = value_colours(&buf, y, 120, app.theme.dim);
            assert_eq!(
                colours.len(),
                1,
                "{label} is one identifier and must be one colour: {colours:?}"
            );
            assert_eq!(
                colours,
                vec![app.theme.fg],
                "{label} is an identifier, not a measurement"
            );
        }
    }

    /// The tab number is part of the navigation key, not a reading.
    #[test]
    fn a_tab_number_keeps_the_tab_bar_colour() {
        let app = fixture_app();
        let buf = buffer_of(&app, 120, 34);
        let bar = row_with(&buf, 120, 34, "Dashboard");
        let colours = row_colours(&buf, bar, 120);
        assert!(
            colours.contains(&active_text_colour(&app.theme)),
            "the active tab keeps its own text colour: {colours:?}"
        );
        assert!(
            !colours.contains(&app.theme.value),
            "the value colour leaked into the tab bar: {colours:?}"
        );
    }

    /// Colour is decoration on top of words that already say the fact: drop the
    /// value colour and the frame reads the same.
    #[test]
    fn the_meaning_survives_without_colour() {
        let mut app = fixture_app();
        app.view = View::Dense;
        let painted = render_to_string(&app, 120, 34);
        let mut plain = fixture_app();
        plain.view = View::Dense;
        plain.theme.value = plain.theme.fg;
        let unpainted = render_to_string(&plain, 120, 34);
        assert_eq!(
            painted, unpainted,
            "removing the value colour changed what the frame says"
        );
        assert!(
            painted.contains("status"),
            "the state is named in words, not only in colour"
        );
    }

    #[test]
    fn the_mask_bitmap_and_the_radio_bars_keep_their_glyphs() {
        // Removing a renderer must not touch the panels that use block glyphs
        // for their own meaning.
        let mut app = fixture_app();
        app.select_tab(Tab::Bands);
        app.bands_supported = (1..=64).collect();
        app.bands_selected = vec![1, 3, 20];
        let bands = render_to_string(&app, 140, 40);
        for glyph in ['█', '▒', '·'] {
            assert!(bands.contains(glyph), "the mask lost {glyph:?}\n{bands}");
        }
        assert!(
            bands.contains("█ ticked"),
            "the mask legend still explains itself\n{bands}"
        );

        app.select_tab(Tab::Dashboard);
        let dash = render_to_string(&app, 120, 42);
        assert!(
            dash.contains("▁▂▃▄▅▆▇█"),
            "the map legend still names its eight levels\n{dash}"
        );
        let radio = dash
            .lines()
            .find(|l| l.contains("RSRP   -9"))
            .unwrap_or_else(|| panic!("no radio gauge row\n{dash}"))
            .to_string();
        assert!(
            radio.chars().any(|c| "▁▂▃▄▅▆▇█".contains(c)),
            "the radio gauge lost its bar: {radio}"
        );
    }

    #[test]
    fn no_panic_from_40x12_to_240x60() {
        let mut app = fixture_app();
        for (w, h) in [
            (40, 12),
            (60, 20),
            (80, 24),
            (120, 34),
            (160, 40),
            (200, 50),
            (240, 60),
        ] {
            for tab in Tab::ALL {
                app.select_tab(tab);
                let text = render_to_string(&app, w, h);
                assert!(!text.is_empty(), "no frame for {tab:?} at {w}x{h}");
            }
        }
    }

    #[test]
    fn bands_mask_draws_all_sixty_four_bits() {
        let mut app = fixture_app();
        app.select_tab(Tab::Bands);
        app.bands_supported = (1..=64).collect();
        app.bands_selected = vec![1, 2, 3];
        let wide = render_to_string(&app, 120, 38);
        let drawn = |t: &str, per_row: usize| {
            t.lines()
                .filter(|l| l.chars().filter(|c| "█▒·".contains(*c)).count() >= per_row)
                .count()
        };
        let rows = drawn(&wide, 32);
        assert_eq!(
            rows, 2,
            "64 bits are two rows of 32 in a wide panel\n{wide}"
        );
        let narrow = render_to_string(&app, 60, 30);
        let narrow_rows = drawn(&narrow, 16);
        assert!(
            narrow_rows >= rows,
            "a narrow panel wraps the mask onto more rows, never fewer\n{narrow}"
        );
    }

    #[test]
    fn bands_chips_mark_only_what_the_device_cannot_do() {
        let mut app = fixture_app();
        app.select_tab(Tab::Bands);
        // B66 is ticked but the device never advertised it; B5 is advertised
        // and unticked, so the mask shows the advertised-not-ticked mark.
        app.bands_supported = vec![1, 3, 5];
        app.bands_selected = vec![1, 66];
        let text = render_to_string(&app, 120, 38);
        assert!(
            text.contains("[B66]!"),
            "a ticked band the device lacks is marked\n{text}"
        );
        assert!(
            text.contains("▒"),
            "an advertised band is not blank\n{text}"
        );
        assert!(
            !text.contains("B5!"),
            "an advertised band is never marked missing\n{text}"
        );
    }

    #[test]
    fn bands_delta_names_both_directions() {
        let mut app = fixture_app();
        app.select_tab(Tab::Bands);
        app.bands_selected.retain(|b| *b != 5);
        app.bands_selected.push(66);
        let text = render_to_string(&app, 120, 38);
        assert!(text.contains("-B5"), "the dropped band is named\n{text}");
        assert!(text.contains("+B66"), "the added band is named\n{text}");
        assert!(
            text.contains("2 changes to apply"),
            "the count is named\n{text}"
        );
    }

    #[test]
    fn bands_write_log_keeps_the_newest_and_caps_its_ring() {
        let mut app = fixture_app();
        app.select_tab(Tab::Bands);
        for i in 0..40 {
            app.note_band_op(format!("op {i}"), "ok".into(), true);
        }
        assert_eq!(app.band_log.len(), crate::app::BAND_LOG_CAP);
        assert_eq!(app.band_log.front().map(|e| e.what.as_str()), Some("op 39"));
        let text = render_to_string(&app, 120, 38);
        assert!(
            text.contains("session only · not persisted"),
            "the log lies about itself\n{text}"
        );
    }

    #[test]
    fn bands_presets_show_two_rows_each_and_the_device_note() {
        let mut app = fixture_app();
        app.select_tab(Tab::Bands);
        app.band_presets = vec![crate::config::BandPreset::new(&[1, 3])];
        app.bands_supported = vec![1, 3];
        let text = render_to_string(&app, 130, 34);
        assert!(
            text.contains("1:B1+B3"),
            "the preset name and index\n{text}"
        );
        assert!(
            text.contains("2 bands · device"),
            "the preset's own count\n{text}"
        );
        assert!(text.contains("PRESETS"), "the region is titled\n{text}");
    }

    #[test]
    fn bands_status_row_carries_the_mask_only_for_bands() {
        let mut app = fixture_app();
        app.select_tab(Tab::Bands);
        let status = render_to_string(&app, 160, 30)
            .lines()
            .last()
            .unwrap_or_default()
            .to_string();
        assert!(
            status.contains(&format!("0x{}", app.band_mask_hex())),
            "the mask a applies belongs on the status row: {status}"
        );
        app.select_tab(Tab::Traffic);
        let other = render_to_string(&app, 160, 30)
            .lines()
            .last()
            .unwrap_or_default()
            .to_string();
        assert!(
            !other.contains(&format!("0x{}", app.band_mask_hex())),
            "the mask is a Bands fact, not a global one: {other}"
        );
    }

    #[test]
    fn bands_signal_strip_follows_the_history_it_is_given() {
        let mut app = fixture_app();
        app.select_tab(Tab::Bands);
        app.history_rsrp.clear();
        assert!(
            !render_to_string(&app, 120, 38).contains("sig  "),
            "no samples, no strip"
        );
        for dbm in [-90.0, -95.0, -100.0] {
            app.history_rsrp.push_back(dbm);
        }
        let text = render_to_string(&app, 120, 38);
        let strip = text
            .lines()
            .find(|l| l.contains("sig  "))
            .expect("the strip is drawn once samples exist");
        const GLYPHS: &str = "▁▂▃▄▅▆▇█";
        let cells = strip
            .split("sig  ")
            .nth(1)
            .unwrap_or_default()
            .chars()
            .take_while(|c| GLYPHS.contains(*c))
            .count();
        assert_eq!(cells, 3, "one cell per sample, no invented points: {strip}");
    }

    #[test]
    fn the_dashboard_counts_incidents_and_never_names_a_cause() {
        let mut app = fixture_app();
        let quiet = render_to_string(&app, 120, 40);
        assert!(
            !quiet.contains(" INC "),
            "a quiet log must not draw an incident line: {quiet}"
        );
        app.incidents = vec![crate::incidents::Incident {
            id: "wan-4242".to_string(),
            kind: crate::incidents::Kind::Wan,
            start_unix: 4242,
            end_unix: 4300,
            evidence: 3,
            confidence: crate::incidents::Confidence::Medium,
            lines: vec!["signal wan: down".to_string()],
            caveat: Some("a read failed within 601 ms of this window".to_string()),
        }];
        let text = render_to_string(&app, 120, 40);
        assert!(text.contains(" INC "), "{text}");
        assert!(text.contains("wan-4242"), "the id must be named: {text}");
        assert!(text.contains("no cause"), "{text}");
    }
}
