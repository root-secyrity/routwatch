//! The seven tab bodies, plus the Lite and Dense views.
//!
//! Every panel is built from `App` only; nothing here touches the network, which
//! is what makes the render tests possible.

use crate::app::{App, View};
use crate::theme::Theme;
use crate::ui::active_text_colour;
use crate::ui::widgets::{bar, bps, bytes, dur, kv, opt_f, opt_s, pad, spark, title_style};
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Paragraph, Wrap};
use ratatui::Frame;

fn truncate(s: &str, n: usize) -> String {
    let count = s.chars().count();
    if count <= n {
        s.to_string()
    } else {
        s.chars().take(n.saturating_sub(1)).collect::<String>() + "…"
    }
}

fn panel(title: String, theme: &Theme) -> Block<'static> {
    Block::bordered()
        .title(Span::styled(format!(" {title} "), title_style(theme.title)))
        .border_style(Style::default().fg(theme.border))
}

pub fn body(f: &mut Frame, area: Rect, app: &App) {
    // Every view owns the body, including on tab 9: the Signal tab's Full layout
    // IS the gauge dashboard, but Lite/Dense must still be able to take over —
    // otherwise `v` looked dead there.
    match app.view {
        View::Lite => lite(f, area, app),
        View::Dense => dense(f, area, app),
        View::Gauges => gauges(f, area, app),
        View::Full if app.tab == crate::app::Tab::Signal => gauges(f, area, app),
        View::Full => match app.tab {
            crate::app::Tab::Dashboard => dashboard(f, area, app),
            crate::app::Tab::Bands => bands_tab(f, area, app),
            crate::app::Tab::Traffic => traffic_tab(f, area, app),
            crate::app::Tab::Network => network_tab(f, area, app),
            crate::app::Tab::Clients => clients_tab(f, area, app),
            crate::app::Tab::Sms => sms_tab(f, area, app),
            crate::app::Tab::Device => device_tab(f, area, app),
            crate::app::Tab::Dns => dns_tab(f, area, app),
            crate::app::Tab::Signal => gauges(f, area, app),
        },
    }
}

// ---------------------------------------------------------------- dashboard

/// Four blocks want 25 rows. Below that each tier drops the least useful block
/// instead of squeezing all four into one row each.
pub fn dashboard(f: &mut Frame, area: Rect, app: &App) {
    if area.height >= 25 {
        dashboard_roomy(f, area, app);
    } else if area.height >= 14 {
        dashboard_compact(f, area, app);
    } else {
        dashboard_tight(f, area, app);
    }
}

/// Minimum height: one panel that still answers "is the link alive?".
fn dashboard_tight(f: &mut Frame, area: Rect, app: &App) {
    let theme = &app.theme;
    f.render_widget(
        Paragraph::new(signal_lines(app))
            .wrap(Wrap { trim: true })
            .block(panel("SIGNAL".into(), theme)),
        area,
    );
}

/// Short terminal: signal, speed and the throughput history; no 5G/connection
/// blocks, no host or error lists — they are a `2`/`5` key away.
fn dashboard_compact(f: &mut Frame, area: Rect, app: &App) {
    let theme = &app.theme;
    let [top, graph] = Layout::vertical([Constraint::Length(8), Constraint::Min(4)]).areas(area);
    let [left, right] =
        Layout::horizontal([Constraint::Percentage(55), Constraint::Percentage(45)]).areas(top);
    f.render_widget(
        Paragraph::new(signal_lines(app))
            .wrap(Wrap { trim: true })
            .block(panel("SIGNAL · 4G".into(), theme)),
        left,
    );
    f.render_widget(
        Paragraph::new(speed_lines(app)).block(panel("SPEED".into(), theme)),
        right,
    );
    let mut lines = vec![Line::from(vec![
        Span::styled("down ", Style::default().fg(theme.dim)),
        Span::styled(bps(last(&app.history_dl)), Style::default().fg(theme.ok)),
    ])];
    lines.extend(spark(
        &app.history_dl,
        graph.width.saturating_sub(8) as usize,
        1,
        app.graph,
        theme.ok,
    ));
    lines.push(Line::from(vec![
        Span::styled("up   ", Style::default().fg(theme.dim)),
        Span::styled(
            bps(last(&app.history_ul)),
            Style::default().fg(theme.accent),
        ),
    ]));
    lines.extend(spark(
        &app.history_ul,
        graph.width.saturating_sub(8) as usize,
        1,
        app.graph,
        theme.accent,
    ));
    f.render_widget(
        Paragraph::new(lines).block(panel("THROUGHPUT".into(), theme)),
        graph,
    );
}

fn dashboard_roomy(f: &mut Frame, area: Rect, app: &App) {
    let theme = &app.theme;
    let [top, mid, graph, bottom] = Layout::vertical([
        Constraint::Length(8),
        Constraint::Length(5),
        Constraint::Length(9),
        Constraint::Min(3),
    ])
    .areas(area);
    let [left, right] =
        Layout::horizontal([Constraint::Percentage(58), Constraint::Percentage(42)]).areas(top);

    f.render_widget(
        Paragraph::new(signal_lines(app))
            .wrap(Wrap { trim: true })
            .block(panel("SIGNAL · 4G".into(), theme)),
        left,
    );
    let speed = Paragraph::new(speed_lines(app)).block(panel("SPEED".into(), theme));
    f.render_widget(speed, right);

    let [a, b] =
        Layout::horizontal([Constraint::Percentage(50), Constraint::Percentage(50)]).areas(mid);
    f.render_widget(
        Paragraph::new(nr_lines(app)).block(panel("5G".into(), theme)),
        a,
    );
    f.render_widget(
        Paragraph::new(connection_lines(app)).block(panel("CONNECTION".into(), theme)),
        b,
    );

    let mut lines = Vec::new();
    lines.push(Line::from(vec![
        Span::styled("down ", Style::default().fg(theme.dim)),
        Span::styled(bps(last(&app.history_dl)), Style::default().fg(theme.ok)),
    ]));
    lines.extend(spark(
        &app.history_dl,
        graph.width.saturating_sub(8) as usize,
        2,
        app.graph,
        theme.ok,
    ));
    lines.push(Line::from(vec![
        Span::styled("up   ", Style::default().fg(theme.dim)),
        Span::styled(
            bps(last(&app.history_ul)),
            Style::default().fg(theme.accent),
        ),
    ]));
    lines.extend(spark(
        &app.history_ul,
        graph.width.saturating_sub(8) as usize,
        2,
        app.graph,
        theme.accent,
    ));
    let style = match app.graph {
        crate::theme::GraphStyle::Bars => "bars",
        crate::theme::GraphStyle::Dots => "dots",
        crate::theme::GraphStyle::Braille => "braille",
    };
    f.render_widget(
        Paragraph::new(lines).block(panel(
            format!("THROUGHPUT · {style} · last 120 samples"),
            theme,
        )),
        graph,
    );

    // Bottom: the two things a router page never shows side by side — hosts and errors.
    let [c, d] =
        Layout::horizontal([Constraint::Percentage(60), Constraint::Percentage(40)]).areas(bottom);
    let mut hosts = vec![Line::from(Span::styled(
        format!("{} {} {}", pad("name", 26), pad("ip", 15), "band"),
        Style::default().fg(theme.dim),
    ))];
    if let Some(s) = &app.snap {
        for c in s
            .clients
            .iter()
            .take(bottom.height.saturating_sub(3) as usize)
        {
            hosts.push(Line::from(vec![
                Span::styled(
                    pad(c.name.as_deref().unwrap_or("(unnamed)"), 26),
                    Style::default().fg(theme.fg),
                ),
                Span::styled(
                    pad(c.ip.as_deref().unwrap_or("—"), 15),
                    Style::default().fg(theme.accent),
                ),
                Span::styled(
                    c.freq.clone().unwrap_or_default(),
                    Style::default().fg(theme.dim),
                ),
            ]));
        }
        if s.clients.is_empty() {
            hosts.push(Line::from(Span::styled(
                "no associated hosts",
                Style::default().fg(theme.dim),
            )));
        }
    }
    f.render_widget(
        Paragraph::new(hosts).block(panel(
            format!(
                "CLIENTS · {}",
                app.snap.as_ref().map(|s| s.clients.len()).unwrap_or(0)
            ),
            theme,
        )),
        c,
    );

    let mut errs = Vec::new();
    for e in app.error_log.iter().take(2) {
        errs.push(Line::from(Span::styled(
            truncate(e, d.width.saturating_sub(4) as usize),
            Style::default().fg(theme.warn),
        )));
    }
    if errs.is_empty() {
        errs.push(Line::from(Span::styled(
            "no errors this session",
            Style::default().fg(theme.dim),
        )));
    }
    f.render_widget(Paragraph::new(errs).block(panel("ERRORS".into(), theme)), d);
}

fn last(v: &std::collections::VecDeque<f64>) -> f64 {
    v.back().copied().unwrap_or(0.0)
}

fn signal_lines(app: &App) -> Vec<Line<'static>> {
    let t = &app.theme;
    let Some(s) = &app.snap else {
        return vec![Line::from(Span::styled(
            "waiting for the first sample…",
            Style::default().fg(t.dim),
        ))];
    };
    let sig = &s.signal;
    let mut v = vec![
        kv(
            "RSRP",
            opt_f(sig.rsrp_dbm, "dBm"),
            Style::default().fg(sig.rsrp_dbm.map(|x| t.rsrp_color(x)).unwrap_or(t.dim)),
            t.dim,
        ),
        kv(
            "RSRQ",
            opt_f(sig.rsrq_db, "dB"),
            Style::default().fg(sig.rsrq_db.map(|x| t.rsrq_color(x)).unwrap_or(t.dim)),
            t.dim,
        ),
        kv(
            "SINR",
            opt_f(sig.sinr_db, "dB"),
            Style::default().fg(sig.sinr_db.map(|x| t.sinr_color(x)).unwrap_or(t.dim)),
            t.dim,
        ),
        kv(
            "RSSI",
            opt_f(sig.rssi_dbm, "dBm"),
            Style::default().fg(t.fg),
            t.dim,
        ),
        kv(
            "band",
            format!(
                "{}  {}  {}",
                sig.band.clone().unwrap_or_else(|| "—".into()),
                match (sig.dl_mhz, sig.ul_mhz) {
                    (Some(d), Some(u)) => format!("{d}/{u} MHz"),
                    _ => "—".into(),
                },
                match (&sig.dl_bw, &sig.ul_bw) {
                    (Some(d), Some(u)) => format!("({d}/{u})"),
                    _ => String::new(),
                }
            ),
            Style::default().fg(t.accent),
            t.dim,
        ),
    ];
    v.push(kv(
        "cell",
        format!(
            "PCI {} · EARFCN {} · {}",
            sig.pci.map(|p| p.to_string()).unwrap_or_else(|| "—".into()),
            sig.earfcn.clone().unwrap_or_else(|| "—".into()),
            sig.tm.clone().unwrap_or_default()
        ),
        Style::default().fg(t.fg),
        t.dim,
    ));
    let secondary = sig
        .dl_mcs_secondary
        .map(|m| format!("/{m}"))
        .unwrap_or_default();
    v.push(kv(
        "mcs",
        format!(
            "DL {}{} · UL {} · CQI {} · quality {}",
            sig.dl_mcs
                .map(|m| m.to_string())
                .unwrap_or_else(|| "—".into()),
            secondary,
            sig.ul_mcs
                .map(|m| m.to_string())
                .unwrap_or_else(|| "—".into()),
            sig.cqi.map(|m| m.to_string()).unwrap_or_else(|| "—".into()),
            app.signal_quality()
        ),
        Style::default().fg(t.fg),
        t.dim,
    ));
    v
}

fn speed_lines(app: &App) -> Vec<Line<'static>> {
    let t = &app.theme;
    let Some(s) = &app.snap else {
        return vec![Line::from("waiting…")];
    };
    let tr = &s.traffic;
    vec![
        Line::from(vec![
            Span::styled("↓ ", Style::default().fg(t.ok)),
            Span::styled(
                bps(tr.dl_rate_bytes as f64),
                Style::default().fg(t.ok).add_modifier(Modifier::BOLD),
            ),
        ]),
        Line::from(vec![
            Span::styled("↑ ", Style::default().fg(t.accent)),
            Span::styled(
                bps(tr.ul_rate_bytes as f64),
                Style::default().fg(t.accent).add_modifier(Modifier::BOLD),
            ),
        ]),
        Line::from(""),
        kv(
            "session",
            format!(
                "{} ↓ / {} ↑ ({})",
                bytes(tr.session_dl_bytes),
                bytes(tr.session_ul_bytes),
                dur(tr.session_secs)
            ),
            Style::default().fg(t.fg),
            t.dim,
        ),
        kv(
            "total",
            format!(
                "{} ↓ / {} ↑ ({})",
                bytes(tr.total_dl_bytes),
                bytes(tr.total_ul_bytes),
                dur(tr.total_secs)
            ),
            Style::default().fg(t.fg),
            t.dim,
        ),
    ]
}

fn nr_lines(app: &App) -> Vec<Line<'static>> {
    let t = &app.theme;
    match app.snap.as_ref().and_then(|s| s.signal.nr.as_ref()) {
        Some(nr) => vec![
            kv(
                "RSRP",
                opt_f(nr.rsrp_dbm, "dBm"),
                Style::default().fg(t.fg),
                t.dim,
            ),
            kv(
                "SINR",
                opt_f(nr.sinr_db, "dB"),
                Style::default().fg(t.fg),
                t.dim,
            ),
            kv(
                "band",
                nr.band.clone().unwrap_or_else(|| "—".into()),
                Style::default().fg(t.accent),
                t.dim,
            ),
        ],
        None => {
            let endc = app
                .snap
                .as_ref()
                .map(|s| s.connection.endc)
                .unwrap_or(false);
            vec![
                Line::from(Span::styled(
                    "not exposed by this firmware",
                    Style::default().fg(t.dim),
                )),
                Line::from(Span::styled(
                    format!(
                        "ENDC {} · NR bars {}",
                        if endc { "on" } else { "off" },
                        app.snap.as_ref().map(|s| s.connection.nr_bars).unwrap_or(0)
                    ),
                    Style::default().fg(t.dim),
                )),
            ]
        }
    }
}

fn connection_lines(app: &App) -> Vec<Line<'static>> {
    let t = &app.theme;
    let Some(s) = &app.snap else {
        return vec![Line::from("waiting…")];
    };
    let c = &s.connection;
    vec![
        kv(
            "network",
            format!("{} ({})", s.plmn.name, s.plmn.numeric),
            Style::default().fg(t.fg),
            t.dim,
        ),
        kv(
            "status",
            format!(
                "{} · {} · ENDC {}",
                c.status,
                c.net_type,
                if c.endc { "on" } else { "off" }
            ),
            Style::default().fg(t.ok),
            t.dim,
        ),
        kv(
            "wan",
            s.device.wan_ip.clone().unwrap_or_else(|| "—".into()),
            Style::default().fg(t.fg),
            t.dim,
        ),
        kv(
            "lan",
            format!("{} · {} clients", s.router, s.clients.len()),
            Style::default().fg(t.fg),
            t.dim,
        ),
    ]
}

// ------------------------------------------------------------------- bands

pub fn bands_tab(f: &mut Frame, area: Rect, app: &App) {
    let t = &app.theme;
    let Some(s) = &app.snap else {
        f.render_widget(
            Paragraph::new("waiting…").block(panel("BANDS".into(), t)),
            area,
        );
        return;
    };

    let mode_label = match app.band_mode.as_str() {
        "00" => "Auto",
        "03" => "4G only",
        "08" => "5G NR",
        other => other,
    };
    let option_label = match app.band_option.as_str() {
        "0" => "NSA",
        "1" => "SA",
        _ => "SA+NSA",
    };
    let current: Vec<String> = s
        .net_mode
        .lte_band
        .as_deref()
        .and_then(crate::router::bands::from_hex)
        .map(crate::router::bands::bands_from_mask)
        .unwrap_or_default()
        .iter()
        .map(|b| format!("B{b}"))
        .collect();

    let mut lines = vec![
        Line::from(vec![
            Span::styled(pad("mode", 9), Style::default().fg(t.dim)),
            Span::styled(
                pad(mode_label, 12),
                Style::default().fg(t.accent).add_modifier(Modifier::BOLD),
            ),
            Span::styled("m cycles auto → 4g → 5g", Style::default().fg(t.dim)),
        ]),
        Line::from(vec![
            Span::styled(pad("nr", 9), Style::default().fg(t.dim)),
            Span::styled(
                pad(
                    if app.band_mode == "08" {
                        option_label
                    } else {
                        "—"
                    },
                    12,
                ),
                Style::default().fg(t.fg),
            ),
            Span::styled(
                if app.band_mode == "08" {
                    "o cycles NSA → SA → SA+NSA"
                } else {
                    "only used in 5G mode"
                },
                Style::default().fg(t.dim),
            ),
        ]),
        Line::from(vec![
            Span::styled(pad("device", 9), Style::default().fg(t.dim)),
            Span::styled(
                format!(
                    "0x{} → {}",
                    s.net_mode.lte_band.clone().unwrap_or_else(|| "—".into()),
                    current.join(", ")
                ),
                Style::default().fg(t.fg),
            ),
        ]),
        Line::from(vec![
            Span::styled(pad("support", 9), Style::default().fg(t.dim)),
            Span::styled(
                format!(
                    "{} bands · all-bands mask {}",
                    s.supported_bands.len(),
                    s.all_bands_mask.clone().unwrap_or_else(|| "unknown".into())
                ),
                Style::default().fg(t.dim),
            ),
        ]),
        Line::from(""),
    ];

    // Band grid: cursor is reverse video, selection is bracketed + accent.
    let width = area.width.saturating_sub(4) as usize;
    let mut row: Vec<Span> = Vec::new();
    let mut row_width = 0usize;
    for (i, band) in s.supported_bands.iter().enumerate() {
        let label = format!("B{band}");
        let selected = app.bands_selected.contains(&(*band as u8));
        let cursor = i == app.band_cursor;
        let text = if selected {
            format!("[{label}]")
        } else {
            format!(" {label} ")
        };
        let w = text.chars().count() + 1;
        if row_width + w > width && !row.is_empty() {
            lines.push(Line::from(std::mem::take(&mut row)));
            row_width = 0;
        }
        let style = if cursor {
            Style::default()
                .fg(active_text_colour(t))
                .bg(t.accent)
                .add_modifier(Modifier::BOLD)
        } else if selected {
            Style::default().fg(t.ok).add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(t.dim)
        };
        row.push(Span::styled(text, style));
        row.push(Span::raw(" "));
        row_width += w;
    }
    if !row.is_empty() {
        lines.push(Line::from(row));
    }

    // Merge presets: ready-made combos and whatever the user saved. The cursor
    // is reverse video; a preset this device cannot do is marked, not hidden.
    if !app.band_presets.is_empty() {
        let mut preset_row: Vec<Span> =
            vec![Span::styled(pad("presets", 9), Style::default().fg(t.dim))];
        let mut preset_width = 8usize;
        for (i, preset) in app.band_presets.iter().enumerate() {
            let missing = app.preset_missing(i);
            let text = if missing.is_empty() {
                format!(" {} ", preset.name)
            } else {
                format!(" {}! ", preset.name)
            };
            let w = text.chars().count() + 1;
            if preset_width + w > width && preset_row.len() > 1 {
                lines.push(Line::from(std::mem::take(&mut preset_row)));
                preset_row.push(Span::styled(pad("", 9), Style::default().fg(t.dim)));
                preset_width = 8;
            }
            let style = if app.preset_cursor == Some(i) {
                Style::default()
                    .fg(active_text_colour(t))
                    .bg(t.accent)
                    .add_modifier(Modifier::BOLD)
            } else if missing.is_empty() {
                Style::default().fg(t.accent)
            } else {
                Style::default().fg(t.dim)
            };
            preset_row.push(Span::styled(text, style));
            preset_row.push(Span::raw(" "));
            preset_width += w;
        }
        lines.push(Line::from(preset_row));
    }

    lines.push(Line::from(""));
    lines.push(Line::from(vec![
        Span::styled(pad("selected", 9), Style::default().fg(t.dim)),
        Span::styled(
            format!(
                "{} bands · {} → mask 0x{}",
                app.bands_selected.len(),
                app.band_merge_label(),
                app.band_mask_hex()
            ),
            Style::default().fg(t.fg).add_modifier(Modifier::BOLD),
        ),
    ]));
    lines.push(Line::from(Span::styled(
        "! = band this device does not list · applies drop the link 10-30s",
        Style::default().fg(t.dim),
    )));
    let write_style = match &app.write {
        crate::write::WriteState::Failed { .. } => Style::default().fg(t.danger),
        crate::write::WriteState::Success { .. } => Style::default().fg(t.ok),
        crate::write::WriteState::Idle => Style::default().fg(t.dim),
        _ => Style::default().fg(t.warn),
    };
    lines.push(Line::from(vec![
        Span::styled(pad("write", 9), Style::default().fg(t.dim)),
        Span::styled(app.write.label(), write_style),
    ]));

    f.render_widget(
        Paragraph::new(lines)
            .block(panel("BANDS".into(), t))
            .wrap(Wrap { trim: false }),
        area,
    );
}

// ----------------------------------------------------------------- traffic

pub fn traffic_tab(f: &mut Frame, area: Rect, app: &App) {
    let t = &app.theme;
    let [top, dns_area] = Layout::vertical([Constraint::Min(8), Constraint::Length(7)]).areas(area);
    let Some(s) = &app.snap else {
        f.render_widget(
            Paragraph::new("waiting…").block(panel("TRAFFIC".into(), t)),
            top,
        );
        return;
    };
    let tr = &s.traffic;
    // The router exposes no monthly plan counter (month-statistics answers 100003),
    // so this bar is explicitly a LOCAL budget over the current session.
    let used_gb = (tr.session_dl_bytes + tr.session_ul_bytes) as f64 / 1024.0 / 1024.0 / 1024.0;
    let frac = if app.budget_gb > 0.0 {
        used_gb / app.budget_gb
    } else {
        0.0
    };
    let colour = if frac > 0.8 {
        t.danger
    } else if frac > 0.5 {
        t.warn
    } else {
        t.ok
    };
    let mut lines = vec![
        Line::from(vec![
            Span::styled("down ", Style::default().fg(t.dim)),
            Span::styled(bps(tr.dl_rate_bytes as f64), Style::default().fg(t.ok).add_modifier(Modifier::BOLD)),
            Span::raw("      "),
            Span::styled("up ", Style::default().fg(t.dim)),
            Span::styled(bps(tr.ul_rate_bytes as f64), Style::default().fg(t.accent).add_modifier(Modifier::BOLD)),
        ]),
        Line::from(""),
        kv("session", format!("{} ↓ / {} ↑ · {}", bytes(tr.session_dl_bytes), bytes(tr.session_ul_bytes), dur(tr.session_secs)), Style::default().fg(t.fg), t.dim),
        kv("total", format!("{} ↓ / {} ↑ · {}", bytes(tr.total_dl_bytes), bytes(tr.total_ul_bytes), dur(tr.total_secs)), Style::default().fg(t.fg), t.dim),
        Line::from(""),
        kv("budget", format!("{used_gb:.1} GB of {:.0} GB session budget (local — no plan counter on this firmware)", app.budget_gb), Style::default().fg(colour), t.dim),
        bar(frac, (top.width.saturating_sub(24)) as usize, colour, t.dim),
    ];
    lines.extend(spark(
        &app.history_dl,
        top.width.saturating_sub(4) as usize,
        3,
        app.graph,
        t.ok,
    ));
    f.render_widget(Paragraph::new(lines).block(panel("TRAFFIC".into(), t)), top);

    // DNS panel — the write target of this tab.
    let mut dns_lines = Vec::new();
    match &s.dns {
        Some(d) => {
            let mode = if d.is_manual() {
                "manual"
            } else {
                "automatic (from DHCP)"
            };
            dns_lines.push(kv(
                "status",
                mode.to_string(),
                Style::default().fg(if d.is_manual() { t.accent } else { t.fg }),
                t.dim,
            ));
            dns_lines.push(kv(
                "primary",
                d.primary.clone().unwrap_or_else(|| "—".into()),
                Style::default().fg(t.fg),
                t.dim,
            ));
            dns_lines.push(kv(
                "secondary",
                d.secondary.clone().unwrap_or_else(|| "—".into()),
                Style::default().fg(t.fg),
                t.dim,
            ));
        }
        None => dns_lines.push(Line::from(Span::styled(
            "dhcp/settings unavailable",
            Style::default().fg(t.dim),
        ))),
    }
    dns_lines.push(Line::from(Span::styled(
        "presets live in tab 8 · writes are verified by a read-back",
        Style::default().fg(t.dim),
    )));
    f.render_widget(
        Paragraph::new(dns_lines).block(panel("DNS".into(), t)),
        dns_area,
    );
}

// ----------------------------------------------------------------- network

pub fn network_tab(f: &mut Frame, area: Rect, app: &App) {
    let t = &app.theme;
    let [top, ping_area] =
        Layout::vertical([Constraint::Min(8), Constraint::Length(7)]).areas(area);
    let Some(s) = &app.snap else {
        f.render_widget(
            Paragraph::new("waiting…").block(panel("NETWORK".into(), t)),
            top,
        );
        return;
    };
    let sig = &s.signal;
    let [left, right] =
        Layout::horizontal([Constraint::Percentage(50), Constraint::Percentage(50)]).areas(top);
    let cell = vec![
        kv(
            "cell id",
            opt_s(&sig.cell_id),
            Style::default().fg(t.fg),
            t.dim,
        ),
        kv(
            "enodeb",
            opt_s(&sig.enodeb),
            Style::default().fg(t.fg),
            t.dim,
        ),
        kv(
            "tac",
            sig.tac.map(|v| v.to_string()).unwrap_or_else(|| "—".into()),
            Style::default().fg(t.fg),
            t.dim,
        ),
        kv(
            "pci",
            sig.pci.map(|v| v.to_string()).unwrap_or_else(|| "—".into()),
            Style::default().fg(t.fg),
            t.dim,
        ),
        kv(
            "earfcn",
            opt_s(&sig.earfcn),
            Style::default().fg(t.fg),
            t.dim,
        ),
        kv(
            "bands",
            format!("{} · {}", opt_s(&sig.dl_bw), opt_s(&sig.ul_bw)),
            Style::default().fg(t.fg),
            t.dim,
        ),
    ];
    f.render_widget(Paragraph::new(cell).block(panel("CELL".into(), t)), left);

    let link = vec![
        kv(
            "operator",
            format!("{} · {}", s.plmn.name, s.plmn.numeric),
            Style::default().fg(t.accent),
            t.dim,
        ),
        kv(
            "type",
            format!("{} ({})", s.connection.net_type, s.connection.net_type_ex),
            Style::default().fg(t.fg),
            t.dim,
        ),
        kv(
            "endc",
            format!(
                "{} · restricted {}",
                if s.connection.endc { "on" } else { "off" },
                s.connection.endc_restricted
            ),
            Style::default().fg(t.fg),
            t.dim,
        ),
        kv(
            "wan",
            opt_s(&s.device.wan_ip),
            Style::default().fg(t.fg),
            t.dim,
        ),
        kv(
            "lan",
            format!("{} · {} clients", s.router, s.clients.len()),
            Style::default().fg(t.fg),
            t.dim,
        ),
        kv(
            "bars",
            format!(
                "LTE {} / NR {}",
                s.connection.signal_bars, s.connection.nr_bars
            ),
            Style::default().fg(t.fg),
            t.dim,
        ),
    ];
    f.render_widget(Paragraph::new(link).block(panel("LINK".into(), t)), right);

    let mut ping_lines = Vec::new();
    match &app.ping {
        Some(p) => {
            ping_lines.push(Line::from(vec![
                Span::styled(format!("{:<16}", p.host), Style::default().fg(t.fg)),
                Span::styled(
                    p.detail.clone(),
                    Style::default().fg(if p.ok { t.ok } else { t.danger }),
                ),
            ]));
            if let Some(avg) = p.avg_ms {
                ping_lines.push(Line::from(Span::styled(
                    format!("rtt avg {avg:.1} ms · packets {}/{}", p.received, p.sent),
                    Style::default().fg(t.dim),
                )));
            }
        }
        None => ping_lines.push(Line::from(Span::styled(
            "no test yet",
            Style::default().fg(t.dim),
        ))),
    }
    ping_lines.push(Line::from(Span::styled(
        "t ping a host from THIS machine (not through the router)",
        Style::default().fg(t.dim),
    )));
    f.render_widget(
        Paragraph::new(ping_lines).block(panel("PING".into(), t)),
        ping_area,
    );
}

// ----------------------------------------------------------------- clients

pub fn clients_tab(f: &mut Frame, area: Rect, app: &App) {
    let t = &app.theme;
    let Some(_) = &app.snap else {
        f.render_widget(
            Paragraph::new("waiting…").block(panel("CLIENTS".into(), t)),
            area,
        );
        return;
    };
    let clients = app.sorted_clients();
    let mut lines = vec![Line::from(Span::styled(
        format!(
            "{} {} {} {} {}",
            pad("name", 26),
            pad("ip", 15),
            pad("mac", 18),
            pad("band", 6),
            "connected"
        ),
        Style::default().fg(t.dim),
    ))];
    for (i, c) in clients.iter().enumerate() {
        let selected = Some(i) == app.selected_client();
        let style = if selected {
            Style::default().fg(t.selected).add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(t.fg)
        };
        lines.push(Line::from(Span::styled(
            format!(
                "{} {} {} {} {} {}",
                if selected { "▶" } else { " " },
                pad(c.name.as_deref().unwrap_or("(unnamed)"), 26),
                pad(c.ip.as_deref().unwrap_or("—"), 15),
                pad(c.mac.as_deref().unwrap_or("—"), 18),
                pad(c.freq.as_deref().unwrap_or("—"), 6),
                dur(c.associated_secs)
            ),
            style,
        )));
    }
    if clients.is_empty() {
        lines.push(Line::from(Span::styled(
            "no associated hosts reported",
            Style::default().fg(t.dim),
        )));
    }
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        format!("sort {}", app.client_sort.name()),
        Style::default().fg(t.dim),
    )));
    f.render_widget(
        Paragraph::new(lines).block(panel(format!("CLIENTS · {}", clients.len()), t)),
        area,
    );
}

// --------------------------------------------------------------------- sms

pub fn sms_tab(f: &mut Frame, area: Rect, app: &App) {
    let t = &app.theme;
    let Some(s) = &app.snap else {
        f.render_widget(
            Paragraph::new("waiting…").block(panel("SMS".into(), t)),
            area,
        );
        return;
    };
    let [counters_area, inbox_area] =
        Layout::vertical([Constraint::Length(8), Constraint::Min(4)]).areas(area);

    let mut lines = Vec::new();
    match &s.sms {
        Some(c) => {
            lines.push(kv(
                "inbox",
                format!("{} ({} unread)", c.inbox, c.unread),
                Style::default().fg(t.fg),
                t.dim,
            ));
            lines.push(kv(
                "outbox",
                c.outbox.to_string(),
                Style::default().fg(t.fg),
                t.dim,
            ));
            lines.push(kv(
                "draft",
                c.draft.to_string(),
                Style::default().fg(t.fg),
                t.dim,
            ));
            lines.push(kv(
                "capacity",
                c.max.to_string(),
                Style::default().fg(t.fg),
                t.dim,
            ));
        }
        None => lines.push(Line::from(Span::styled(
            "counters unavailable",
            Style::default().fg(t.dim),
        ))),
    }
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        "sending is not enabled in this build",
        Style::default().fg(t.dim),
    )));
    f.render_widget(
        Paragraph::new(lines).block(panel("COUNTERS".into(), t)),
        counters_area,
    );

    let mut body = Vec::new();
    match &app.extras {
        None => body.push(Line::from(Span::styled(
            if app.extras_requested {
                "reading inbox…"
            } else {
                "press R to read the inbox"
            },
            Style::default().fg(t.dim),
        ))),
        Some(x) => {
            if let Some(err) = &x.sms_error {
                body.push(Line::from(Span::styled(
                    format!("blocked: {err}"),
                    Style::default().fg(t.warn).add_modifier(Modifier::BOLD),
                )));
                body.push(Line::from(""));
                body.push(Line::from(Span::styled(
                    "the counters work, the message bodies do not: this firmware refuses",
                    Style::default().fg(t.fg),
                )));
                body.push(Line::from(Span::styled(
                    "sms/sms-list for the API session (error 100005).",
                    Style::default().fg(t.fg),
                )));
                body.push(Line::from(""));
                body.push(Line::from(Span::styled(
                    "read them at http://192.168.8.1 → SMS, or with the HUAWEI AI Life app.",
                    Style::default().fg(t.accent),
                )));
            } else if x.sms_inbox.is_empty() {
                body.push(Line::from(Span::styled(
                    "inbox is empty",
                    Style::default().fg(t.dim),
                )));
            } else {
                body.push(Line::from(Span::styled(
                    format!("{:<8} {:<16} {:<18} {}", "index", "phone", "date", "text"),
                    Style::default().fg(t.dim),
                )));
                for m in x
                    .sms_inbox
                    .iter()
                    .take(inbox_area.height.saturating_sub(2) as usize)
                {
                    body.push(Line::from(vec![
                        Span::styled(
                            if m.unread { "● " } else { "  " },
                            Style::default().fg(if m.unread { t.accent } else { t.dim }),
                        ),
                        Span::styled(
                            pad(m.phone.as_deref().unwrap_or("—"), 16),
                            Style::default().fg(t.fg),
                        ),
                        Span::styled(
                            pad(m.date.as_deref().unwrap_or("—"), 18),
                            Style::default().fg(t.dim),
                        ),
                        Span::styled(
                            truncate(
                                m.content.as_deref().unwrap_or(""),
                                inbox_area.width.saturating_sub(48) as usize,
                            ),
                            Style::default().fg(t.fg),
                        ),
                    ]));
                }
            }
        }
    }
    f.render_widget(
        Paragraph::new(body).block(panel("INBOX".into(), t)),
        inbox_area,
    );
}

// --------------------------------------------------------------------- dns

pub fn dns_tab(f: &mut Frame, area: Rect, app: &App) {
    let t = &app.theme;
    let current = app.snap.as_ref().and_then(|s| s.dns.as_ref());
    let mut lines = Vec::new();
    match current {
        Some(d) => {
            let mode = if d.is_manual() {
                "manual"
            } else {
                "automatic (from DHCP)"
            };
            lines.push(Line::from(vec![
                Span::styled(pad("current", 10), Style::default().fg(t.dim)),
                Span::styled(
                    pad(mode, 24),
                    Style::default().fg(if d.is_manual() { t.accent } else { t.ok }),
                ),
                Span::styled(
                    format!(
                        "{} / {}",
                        d.primary.clone().unwrap_or_else(|| "—".into()),
                        d.secondary.clone().unwrap_or_else(|| "—".into())
                    ),
                    Style::default().fg(t.fg),
                ),
            ]));
        }
        None => lines.push(Line::from(Span::styled(
            "dhcp/settings unavailable on this firmware",
            Style::default().fg(t.danger),
        ))),
    }
    lines.push(Line::from(""));

    for (i, (name, preset)) in crate::app::DNS_PRESETS.iter().enumerate() {
        let selected = i == app.dns_preset;
        let style = if selected {
            Style::default()
                .fg(active_text_colour(t))
                .bg(t.accent)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(t.fg)
        };
        let detail = match preset {
            crate::app::DnsPreset::Automatic => {
                "keep the servers the DHCP server hands out".to_string()
            }
            crate::app::DnsPreset::Servers(primary, secondary) => {
                format!("{primary} / {secondary}")
            }
            crate::app::DnsPreset::Custom => {
                "opens the prompt: type any two IPv4 servers".to_string()
            }
        };
        lines.push(Line::from(Span::styled(
            format!("{} {}", if selected { "▶" } else { " " }, pad(name, 44)),
            style,
        )));
        lines.push(Line::from(Span::styled(
            format!("    {}", pad(&detail, 52)),
            Style::default().fg(t.dim),
        )));
    }

    lines.push(Line::from(""));
    let write_style = match &app.write {
        crate::write::WriteState::Failed { .. } => Style::default().fg(t.danger),
        crate::write::WriteState::Success { .. } => Style::default().fg(t.ok),
        crate::write::WriteState::Idle => Style::default().fg(t.dim),
        _ => Style::default().fg(t.warn),
    };
    lines.push(Line::from(vec![
        Span::styled(pad("write", 10), Style::default().fg(t.dim)),
        Span::styled(app.write.label(), write_style),
    ]));
    lines.push(Line::from(Span::styled(
        "every write is verified by reading dhcp/settings back",
        Style::default().fg(t.dim),
    )));

    f.render_widget(Paragraph::new(lines).block(panel("DNS".into(), t)), area);
}

// ------------------------------------------------------------------ device

pub fn device_tab(f: &mut Frame, area: Rect, app: &App) {
    let t = &app.theme;
    let Some(s) = &app.snap else {
        f.render_widget(
            Paragraph::new("waiting…").block(panel("DEVICE".into(), t)),
            area,
        );
        return;
    };
    let d = &s.device;
    let [info_area, log_area] =
        Layout::vertical([Constraint::Min(12), Constraint::Length(9)]).areas(area);

    let mut lines = vec![
        kv(
            "model",
            opt_s(&d.name),
            Style::default().fg(t.accent),
            t.dim,
        ),
        kv(
            "software",
            opt_s(&d.software),
            Style::default().fg(t.fg),
            t.dim,
        ),
        kv("webui", opt_s(&d.webui), Style::default().fg(t.fg), t.dim),
        kv(
            "hardware",
            opt_s(&d.hardware),
            Style::default().fg(t.fg),
            t.dim,
        ),
        kv("serial", opt_s(&d.serial), Style::default().fg(t.fg), t.dim),
        kv("imei", opt_s(&d.imei), Style::default().fg(t.fg), t.dim),
        kv("wan ip", opt_s(&d.wan_ip), Style::default().fg(t.fg), t.dim),
        kv(
            "uptime",
            d.uptime_secs.map(dur).unwrap_or_else(|| "—".into()),
            Style::default().fg(t.fg),
            t.dim,
        ),
    ];
    match app.extras.as_ref() {
        Some(x) => {
            if let Some(pin) = &x.pin {
                lines.push(kv(
                    "sim",
                    format!(
                        "state {} · pin opt {} · retries {} / puk {}",
                        pin.sim_state.clone().unwrap_or_else(|| "—".into()),
                        pin.pin_state.clone().unwrap_or_else(|| "—".into()),
                        pin.pin_retries.clone().unwrap_or_else(|| "—".into()),
                        pin.puk_retries.clone().unwrap_or_else(|| "—".into())
                    ),
                    Style::default().fg(if pin.locked() { t.danger } else { t.fg }),
                    t.dim,
                ));
            }
            if let Some(md) = x.mobile_data {
                lines.push(kv(
                    "mobile",
                    if md {
                        "data on".into()
                    } else {
                        "data off".into()
                    },
                    Style::default().fg(if md { t.ok } else { t.warn }),
                    t.dim,
                ));
            }
        }
        None => lines.push(Line::from(Span::styled(
            if app.extras_requested {
                "reading SIM state…"
            } else {
                "press R for SIM / data state"
            },
            Style::default().fg(t.dim),
        ))),
    }
    lines.push(Line::from(""));
    f.render_widget(
        Paragraph::new(lines).block(panel("DEVICE".into(), t)),
        info_area,
    );

    let mut log_lines = Vec::new();
    if app.event_log.is_empty() {
        log_lines.push(Line::from(Span::styled(
            "no events this session",
            Style::default().fg(t.dim),
        )));
    }
    for e in app
        .event_log
        .iter()
        .take(log_area.height.saturating_sub(2) as usize)
    {
        let colour = match e.level {
            crate::app::LogLevel::Info => t.fg,
            crate::app::LogLevel::Warn => t.warn,
            crate::app::LogLevel::Error => t.danger,
        };
        log_lines.push(Line::from(vec![
            Span::styled(
                format!("{:<8}", format!("+{}s", e.at)),
                Style::default().fg(t.dim),
            ),
            Span::styled(
                truncate(&e.msg, log_area.width.saturating_sub(10) as usize),
                Style::default().fg(colour),
            ),
        ]));
    }
    f.render_widget(
        Paragraph::new(log_lines).block(panel(format!("EVENTS · {}", app.event_log.len()), t)),
        log_area,
    );
}

// -------------------------------------------------------------------- gauges

pub fn gauges(f: &mut Frame, area: Rect, app: &App) {
    let Some(s) = &app.snap else {
        f.render_widget(Paragraph::new("waiting for the first sample…"), area);
        return;
    };
    if area.height < 12 || area.width < 40 {
        f.render_widget(
            Paragraph::new("terminal too small for gauges — press V"),
            area,
        );
        return;
    }
    let wide = area.width >= 90;
    let top_h = if wide { 9 } else { 7 };
    let [top, strip, graphs] = Layout::vertical([
        Constraint::Length(top_h),
        Constraint::Length(2),
        Constraint::Min(5),
    ])
    .areas(area);
    let split = if wide { 64 } else { 58 };
    let [sig_area, spd_area] = Layout::horizontal([
        Constraint::Percentage(split),
        Constraint::Percentage(100 - split),
    ])
    .areas(top);

    signal_panel(f, sig_area, app, s, wide);
    speed_panel(f, spd_area, app, wide);
    network_strip(f, strip, app, s);
    graphs_panel(f, graphs, app, wide);
}

/// One signal metric: value, physical domain and its two 3GPP thresholds.
struct Metric {
    label: &'static str,
    value: Option<f32>,
    min: f32,
    max: f32,
    poor_until: f32,
    fair_until: f32,
    unit: &'static str,
}

fn signal_panel(f: &mut Frame, area: Rect, app: &App, s: &crate::model::Snapshot, wide: bool) {
    let t = &app.theme;
    let kind = crate::app::resolve_signal_panel(Some(s));
    let title = match kind {
        crate::app::SignalPanel::LteGauges => "SIGNAL · 4G",
        crate::app::SignalPanel::NrGauges => "SIGNAL · 5G NR",
        crate::app::SignalPanel::NrBars => "SIGNAL · 5G (bars only)",
    };
    let block = panel(title.to_string(), t);
    let inner = block.inner(area);
    f.render_widget(block, area);

    if kind == crate::app::SignalPanel::NrBars {
        let bars = s.connection.nr_bars.min(5) as usize;
        let mut lines = vec![Line::from(vec![
            Span::styled("NR bars ", Style::default().fg(t.dim)),
            Span::styled(
                "█".repeat(bars) + &"░".repeat(5 - bars),
                Style::default().fg(if bars >= 3 { t.ok } else { t.warn }),
            ),
            Span::styled(
                format!(
                    "  {bars}/5 · ENDC {}",
                    if s.connection.endc { "on" } else { "off" }
                ),
                Style::default().fg(t.fg),
            ),
        ])];
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            "this firmware publishes 5G as bars only — no NR RSRP/SINR values",
            Style::default().fg(t.warn),
        )));
        lines.push(Line::from(Span::styled(
            "the panel switches to full 5G gauges automatically if a firmware ever sends them",
            Style::default().fg(t.dim),
        )));
        f.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), inner);
        return;
    }

    let nr = s.signal.nr.clone().unwrap_or_default();
    let fiveg = kind == crate::app::SignalPanel::NrGauges;
    let rows: Vec<Metric> = if fiveg {
        vec![
            Metric {
                label: "RSRP",
                value: nr.rsrp_dbm,
                min: -140.0,
                max: -44.0,
                poor_until: -100.0,
                fair_until: -80.0,
                unit: "dBm",
            },
            Metric {
                label: "RSRQ",
                value: nr.rsrq_db,
                min: -20.0,
                max: -3.0,
                poor_until: -15.0,
                fair_until: -10.0,
                unit: "dB",
            },
            Metric {
                label: "SINR",
                value: nr.sinr_db,
                min: -10.0,
                max: 30.0,
                poor_until: 10.0,
                fair_until: 20.0,
                unit: "dB",
            },
        ]
    } else {
        vec![
            Metric {
                label: "RSRP",
                value: s.signal.rsrp_dbm,
                min: -140.0,
                max: -44.0,
                poor_until: -100.0,
                fair_until: -80.0,
                unit: "dBm",
            },
            Metric {
                label: "RSRQ",
                value: s.signal.rsrq_db,
                min: -20.0,
                max: -3.0,
                poor_until: -15.0,
                fair_until: -10.0,
                unit: "dB",
            },
            Metric {
                label: "SINR",
                value: s.signal.sinr_db,
                min: -10.0,
                max: 30.0,
                poor_until: 10.0,
                fair_until: 20.0,
                unit: "dB",
            },
            Metric {
                label: "RSSI",
                value: s.signal.rssi_dbm,
                min: -113.0,
                max: -51.0,
                poor_until: -95.0,
                fair_until: -75.0,
                unit: "dBm",
            },
        ]
    };

    let label_w = 6usize;
    let val_w = 11usize;
    let qual_w = 6usize;
    let ruler_w = (inner.width as usize)
        .saturating_sub(label_w + val_w + qual_w + 3)
        .max(8);
    let mut lines: Vec<Line> = Vec::new();
    for m in rows.iter() {
        let colour = match m.label {
            "RSRP" => m.value.map(|v| t.rsrp_color(v)).unwrap_or(t.dim),
            "RSRQ" => m.value.map(|v| t.rsrq_color(v)).unwrap_or(t.dim),
            "SINR" => m.value.map(|v| t.sinr_color(v)).unwrap_or(t.dim),
            _ => m.value.map(|v| t.rsrp_color(v)).unwrap_or(t.dim),
        };
        let qual = quality_label(m.label, m.value);
        let mut spans = vec![
            Span::styled(pad(m.label, label_w), Style::default().fg(t.dim)),
            Span::styled(
                pad(
                    &m.value
                        .map(|v| format!("{v:.0} {}", m.unit))
                        .unwrap_or_else(|| "—".to_string()),
                    val_w,
                ),
                Style::default().fg(colour).add_modifier(Modifier::BOLD),
            ),
            Span::styled(pad(qual, qual_w), Style::default().fg(colour)),
        ];
        if let Some(v) = m.value {
            let spec = crate::ui::widgets::RulerSpec {
                min: m.min as f64,
                max: m.max as f64,
                poor_until: m.poor_until as f64,
                fair_until: m.fair_until as f64,
                width: ruler_w,
            };
            spans.extend(crate::ui::widgets::ruler(v as f64, &spec, t.ok, t.warn, t.danger).spans);
        } else {
            spans.push(Span::styled(
                "no reading from this firmware".to_string(),
                Style::default().fg(t.dim),
            ));
        }
        lines.push(Line::from(spans));
    }

    lines.push(Line::from(""));
    let mut facts = vec![Span::styled(
        format!(" {}", s.plmn.name),
        Style::default().fg(t.accent).add_modifier(Modifier::BOLD),
    )];
    let detail = if fiveg {
        format!(
            " · {} · N-band {} · {} MHz · DL MCS {} · PCI {}",
            s.connection.net_type,
            nr.band.clone().unwrap_or_else(|| "—".into()),
            nr.dl_mhz
                .map(|v| v.to_string())
                .unwrap_or_else(|| "—".into()),
            nr.dl_mcs
                .map(|v| v.to_string())
                .unwrap_or_else(|| "—".into()),
            s.signal
                .pci
                .map(|p| p.to_string())
                .unwrap_or_else(|| "—".into()),
        )
    } else {
        format!(
            " · {} · {} · {} MHz · {} · PCI {} · CQI {}",
            s.connection.net_type,
            s.signal.band.clone().unwrap_or_else(|| "—".into()),
            s.signal
                .dl_mhz
                .map(|v| v.to_string())
                .unwrap_or_else(|| "—".into()),
            s.signal.tm.clone().unwrap_or_else(|| "—".into()),
            s.signal
                .pci
                .map(|p| p.to_string())
                .unwrap_or_else(|| "—".into()),
            s.signal
                .cqi
                .map(|v| v.to_string())
                .unwrap_or_else(|| "—".into()),
        )
    };
    facts.push(Span::styled(detail, Style::default().fg(t.dim)));
    lines.push(Line::from(facts));

    if wide && inner.height as usize > lines.len() + 1 {
        lines.push(Line::from(""));
        lines.push(Line::from(vec![
            Span::styled(" quality ", Style::default().fg(t.dim)),
            Span::styled(
                "▮".repeat(s.connection.nr_bars.min(5) as usize)
                    + &"▯".repeat(5 - s.connection.nr_bars.min(5) as usize),
                Style::default().fg(if s.connection.nr_bars >= 3 {
                    t.ok
                } else {
                    t.warn
                }),
            ),
            Span::styled(
                format!(
                    "  {} bars · ENDC {} · the ruler zones are the 3GPP thresholds",
                    s.connection.nr_bars,
                    if s.connection.endc { "on" } else { "off" }
                ),
                Style::default().fg(t.dim),
            ),
        ]));
    }
    f.render_widget(Paragraph::new(lines), inner);
}

fn quality_label(metric: &str, value: Option<f32>) -> &'static str {
    let Some(v) = value else { return "n/a" };
    match metric {
        "RSRP" => {
            if v >= -80.0 {
                "good"
            } else if v >= -100.0 {
                "fair"
            } else {
                "poor"
            }
        }
        "RSRQ" => {
            if v >= -10.0 {
                "good"
            } else if v >= -15.0 {
                "fair"
            } else {
                "poor"
            }
        }
        "SINR" => {
            if v >= 20.0 {
                "good"
            } else if v >= 10.0 {
                "fair"
            } else {
                "poor"
            }
        }
        "RSSI" => {
            if v >= -75.0 {
                "good"
            } else if v >= -95.0 {
                "fair"
            } else {
                "poor"
            }
        }
        _ => "",
    }
}

fn speed_panel(f: &mut Frame, area: Rect, app: &App, wide: bool) {
    let t = &app.theme;
    let Some(s) = &app.snap else { return };
    let block = panel("SPEED".into(), t);
    let inner = block.inner(area);
    f.render_widget(block, area);

    let dl = s.traffic.dl_rate_bytes as f64;
    let ul = s.traffic.ul_rate_bytes as f64;
    let peak = app
        .history_dl
        .iter()
        .chain(app.history_ul.iter())
        .fold(0.0_f64, |a, b| a.max(*b))
        .max(100_000.0);

    let mut text_top = inner.y;
    if wide && inner.height >= 7 {
        let gauges = Rect { height: 4, ..inner };
        let rects = Layout::horizontal([Constraint::Percentage(50), Constraint::Percentage(50)])
            .split(gauges);
        for (i, (label, value, colour)) in [("down", dl, t.ok), ("up", ul, t.accent)]
            .iter()
            .enumerate()
        {
            let mut body = crate::ui::widgets::radial_gauge(
                *value,
                0.0,
                peak * 1.1,
                (rects[i].width as usize).saturating_sub(2).min(10),
                3,
                *colour,
                t.border,
            );
            body.push(Line::from(Span::styled(
                format!("{label} {}", bps(*value)),
                Style::default().fg(*colour).add_modifier(Modifier::BOLD),
            )));
            f.render_widget(
                Paragraph::new(body).alignment(ratatui::layout::Alignment::Center),
                rects[i],
            );
        }
        text_top += 4;
    }

    let rest = Rect {
        y: text_top,
        height: inner.height.saturating_sub(text_top - inner.y),
        ..inner
    };
    let mut lines = Vec::new();
    if !(wide && inner.height >= 7) {
        lines.push(Line::from(vec![
            Span::styled("↓ ", Style::default().fg(t.ok)),
            Span::styled(
                bps(dl),
                Style::default().fg(t.ok).add_modifier(Modifier::BOLD),
            ),
            Span::raw("   "),
            Span::styled("↑ ", Style::default().fg(t.accent)),
            Span::styled(
                bps(ul),
                Style::default().fg(t.accent).add_modifier(Modifier::BOLD),
            ),
        ]));
    }
    lines.push(Line::from(Span::styled(
        format!(
            "session {} ↓ · {} ↑",
            bytes(s.traffic.session_dl_bytes),
            bytes(s.traffic.session_ul_bytes)
        ),
        Style::default().fg(t.dim),
    )));
    lines.extend(spark(
        &app.history_dl,
        rest.width.saturating_sub(2) as usize,
        (rest.height as usize).saturating_sub(2).clamp(1, 3),
        app.graph,
        t.ok,
    ));
    f.render_widget(Paragraph::new(lines), rest);
}

fn network_strip(f: &mut Frame, area: Rect, app: &App, s: &crate::model::Snapshot) {
    let t = &app.theme;
    let line = Line::from(vec![
        Span::styled(
            format!(" {} ", s.plmn.name),
            Style::default().fg(t.accent).add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            format!(
                "{} · {} · ENDC {} · {} · {} MHz · PCI {} · {} · {} clients",
                s.plmn.numeric,
                s.connection.net_type,
                if s.connection.endc { "on" } else { "off" },
                s.signal.band.clone().unwrap_or_else(|| "—".into()),
                s.signal
                    .dl_mhz
                    .map(|v| v.to_string())
                    .unwrap_or_else(|| "—".into()),
                s.signal
                    .pci
                    .map(|p| p.to_string())
                    .unwrap_or_else(|| "—".into()),
                s.signal.tm.clone().unwrap_or_else(|| "—".into()),
                s.clients.len()
            ),
            Style::default().fg(t.fg),
        ),
    ]);
    f.render_widget(Paragraph::new(line), area);
}

fn graphs_panel(f: &mut Frame, area: Rect, app: &App, wide: bool) {
    let t = &app.theme;
    let block = panel("SIGNAL GRAPHS · last 120 samples".into(), t);
    let inner = block.inner(area);
    f.render_widget(block, area);

    // Fixed physical domains: the same reading always sits at the same height,
    // and one sample is still visible (auto-scaling hid both facts).
    let series = [
        (
            "RSRP",
            &app.history_rsrp,
            t.rsrp_color(-90.0),
            -140.0,
            -44.0,
        ),
        ("RSRQ", &app.history_rsrq, t.rsrq_color(-9.0), -20.0, -3.0),
        ("SINR", &app.history_sinr, t.sinr_color(12.0), -10.0, 30.0),
    ];

    // Allocate rows so that all three series always render, even on a short
    // terminal: three label lines, the rest shared between the traces.
    let avail = (inner.height as usize).saturating_sub(3);
    let rows = (avail / 3).clamp(1, 4);
    let width = (inner.width as usize).saturating_sub(14).max(8);

    let mut lines: Vec<Line> = Vec::new();
    for (label, hist, colour, min, max) in series {
        lines.push(Line::from(vec![
            Span::styled(pad(label, 6), Style::default().fg(t.dim)),
            Span::styled(
                pad(
                    &hist
                        .back()
                        .map(|v| format!("{v:.0}"))
                        .unwrap_or_else(|| "—".to_string()),
                    6,
                ),
                Style::default().fg(colour).add_modifier(Modifier::BOLD),
            ),
            Span::styled(format!("{min:.0}…{max:.0}"), Style::default().fg(t.dim)),
        ]));
        lines.extend(crate::ui::widgets::spark_scaled(
            hist, width, rows, app.graph, colour, min, max,
        ));
    }
    if wide && inner.height as usize > lines.len() {
        lines.push(Line::from(Span::styled(
            "fixed zones: RSRP -140…-44 dBm · RSRQ -20…-3 dB · SINR -10…30 dB",
            Style::default().fg(t.dim),
        )));
    }
    f.render_widget(Paragraph::new(lines), inner);
}

// -------------------------------------------------------------- lite/dense

pub fn lite(f: &mut Frame, area: Rect, app: &App) {
    let t = &app.theme;
    let Some(s) = &app.snap else {
        f.render_widget(Paragraph::new("waiting for the first sample…"), area);
        return;
    };
    let sig = &s.signal;
    let lines = vec![
        Line::from(vec![
            Span::styled("RSRP ", Style::default().fg(t.dim)),
            Span::styled(
                opt_f(sig.rsrp_dbm, "dBm"),
                Style::default().fg(sig.rsrp_dbm.map(|v| t.rsrp_color(v)).unwrap_or(t.dim)),
            ),
            Span::styled(
                format!("  {}  ", app.signal_quality()),
                Style::default().fg(t.dim),
            ),
            Span::styled(
                sig.band.clone().unwrap_or_default(),
                Style::default().fg(t.accent),
            ),
        ]),
        Line::from(vec![
            Span::styled("↓ ", Style::default().fg(t.ok)),
            Span::styled(
                bps(s.traffic.dl_rate_bytes as f64),
                Style::default().fg(t.ok),
            ),
            Span::raw("   "),
            Span::styled("↑ ", Style::default().fg(t.accent)),
            Span::styled(
                bps(s.traffic.ul_rate_bytes as f64),
                Style::default().fg(t.accent),
            ),
        ]),
        Line::from(vec![
            Span::styled("session ", Style::default().fg(t.dim)),
            Span::styled(
                format!("{} ↓", bytes(s.traffic.session_dl_bytes)),
                Style::default().fg(t.fg),
            ),
        ]),
        Line::from(vec![
            Span::styled("clients ", Style::default().fg(t.dim)),
            Span::styled(s.clients.len().to_string(), Style::default().fg(t.fg)),
            Span::styled("   network ", Style::default().fg(t.dim)),
            Span::styled(
                format!("{} {}", s.plmn.name, s.connection.net_type),
                Style::default().fg(t.fg),
            ),
        ]),
        Line::from(vec![
            Span::styled("SINR ", Style::default().fg(t.dim)),
            Span::styled(
                opt_f(sig.sinr_db, "dB"),
                Style::default().fg(sig.sinr_db.map(|v| t.sinr_color(v)).unwrap_or(t.dim)),
            ),
            Span::styled("   ENDC ", Style::default().fg(t.dim)),
            Span::styled(
                if s.connection.endc { "on" } else { "off" },
                Style::default().fg(t.fg),
            ),
        ]),
        Line::from(Span::styled(
            "V full · L toggle · ? help · q quit",
            Style::default().fg(t.dim),
        )),
    ];
    f.render_widget(Paragraph::new(lines), area);
}

pub fn dense(f: &mut Frame, area: Rect, app: &App) {
    let t = &app.theme;
    let rows =
        Layout::vertical([Constraint::Percentage(50), Constraint::Percentage(50)]).split(area);
    let top =
        Layout::horizontal([Constraint::Percentage(50), Constraint::Percentage(50)]).split(rows[0]);
    let bottom =
        Layout::horizontal([Constraint::Percentage(50), Constraint::Percentage(50)]).split(rows[1]);

    f.render_widget(
        Paragraph::new(signal_lines(app)).block(panel("SIGNAL".into(), t)),
        top[0],
    );
    f.render_widget(
        Paragraph::new(speed_lines(app)).block(panel("SPEED".into(), t)),
        top[1],
    );
    f.render_widget(
        Paragraph::new(nr_lines(app)).block(panel("5G".into(), t)),
        bottom[0],
    );
    f.render_widget(
        Paragraph::new(connection_lines(app)).block(panel("CONNECTION".into(), t)),
        bottom[1],
    );
}
