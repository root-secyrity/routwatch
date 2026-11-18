//! The nine tab bodies, plus the Lite and Dense views.
//!
//! Every panel is built from `App` only; nothing here touches the network, which
//! is what makes the render tests possible.

use crate::app::{App, View};
use crate::theme::Theme;
use crate::ui::active_text_colour;
use crate::ui::widgets::{bar, bitrate, bps, bytes, dur, kv, opt_f, opt_s, pad, title_style};
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Clear, Paragraph, Wrap};
use ratatui::Frame;
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

pub(crate) fn truncate(s: &str, n: usize) -> String {
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

pub(crate) fn panel(title: String, theme: &Theme) -> Block<'static> {
    Block::bordered()
        .title(Span::styled(format!(" {title} "), title_style(theme.title)))
        .border_style(Style::default().fg(theme.border))
}

pub fn body(f: &mut Frame, area: Rect, app: &App) {
    // Every view owns the body, including on tab 9: the Signal tab's Full layout
    // IS the gauge dashboard, but Lite/Dense must still be able to take over  -
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
    // The instrument band needs its own rows; without them the blocks below
    // keep the whole area rather than every panel losing a row.
    if area.height >= crate::ui::tiles::HEIGHT + 20 {
        let [band, rest] = Layout::vertical([
            Constraint::Length(crate::ui::tiles::HEIGHT),
            Constraint::Min(20),
        ])
        .areas(area);
        crate::ui::tiles::instruments(f, band, app);
        dashboard_blocks(f, rest, app);
        return;
    }
    dashboard_blocks(f, area, app);
}

fn dashboard_blocks(f: &mut Frame, area: Rect, app: &App) {
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
/// blocks, no host or error lists - they are a `2`/`5` key away.
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
    throughput_panel(f, graph, app);
}

/// Name of the active graph style, for the panel title.
fn graph_style_name(style: crate::theme::GraphStyle) -> &'static str {
    match style {
        crate::theme::GraphStyle::Bars => "bars",
        crate::theme::GraphStyle::Dots => "dots",
        crate::theme::GraphStyle::Braille => "braille",
        crate::theme::GraphStyle::Ascii => "ascii",
    }
}

/// The time window a plot draws: the record capacity times the sample period.
///
/// A plot is sized by this window and the panel, never by the samples on hand:
/// the same span of seconds must cover the same columns whether the record is
/// empty, half full or full, and a tick names an instant inside the window.
fn plot_window(app: &App) -> u64 {
    (crate::app::HISTORY as u64 * app.refresh_ms.max(1) / 1000).max(1)
}

/// The throughput plot: rx above the zero row, tx below it, one scale, one
/// time window, and an axis that names the window actually drawn.
fn throughput_panel(f: &mut Frame, area: Rect, app: &App) {
    let theme = &app.theme;
    let rx: Vec<u64> = app.history_dl.iter().map(|v| v.max(0.0) as u64).collect();
    let tx: Vec<u64> = app.history_ul.iter().map(|v| v.max(0.0) as u64).collect();
    let samples = rx.len().max(tx.len());
    let secs = (samples as u64 * app.refresh_ms.max(1) / 1000).max(1);

    // No sample means no window: a "last 1s" title would be a claim about data
    // that does not exist yet.
    let title = match samples {
        0 => format!("THROUGHPUT · {}", graph_style_name(app.graph)),
        // One sample is a reading, not a window: naming a span here was a claim
        // about data that does not exist yet.
        1 => format!(
            "THROUGHPUT · {} · 1 sample, collecting",
            graph_style_name(app.graph)
        ),
        _ => format!(
            "THROUGHPUT · {} · last {secs}s",
            graph_style_name(app.graph)
        ),
    };
    let block = panel(title, theme);
    let inner = block.inner(area);
    f.render_widget(block, area);
    if samples == 0 {
        f.render_widget(
            Paragraph::new(Line::from(Span::styled(
                "waiting for the first sample",
                Style::default().fg(theme.dim),
            ))),
            inner,
        );
        return;
    }

    if inner.height < 4 || inner.width < 16 {
        // No room for a plot: one honest line beats a broken one.
        f.render_widget(
            Paragraph::new(Line::from(Span::styled(
                format!(
                    "down {} · up {}",
                    bps(last(&app.history_dl)),
                    bps(last(&app.history_ul))
                ),
                Style::default().fg(theme.dim),
            ))),
            inner,
        );
        return;
    }

    mirrored_body(f, inner, app);
}

/// The mirrored download/upload body, starting at `inner.y`. The caller owns the
/// block, the empty state and the "no room" line, because the Dashboard and the
/// Signal tab phrase those differently.
fn mirrored_body(f: &mut Frame, inner: Rect, app: &App) {
    let theme = &app.theme;
    let opts = app.chart_opts();
    let rx: Vec<u64> = app.history_dl.iter().map(|v| v.max(0.0) as u64).collect();
    let tx: Vec<u64> = app.history_ul.iter().map(|v| v.max(0.0) as u64).collect();
    let samples = rx.len().max(tx.len());
    let window = plot_window(app);
    let record_secs = (samples as u64 * app.refresh_ms.max(1) / 1000).max(1);
    let secs_per_sample = (app.refresh_ms.max(1) / 1000).max(1);
    // One sample is not a trend: it keeps the single column it really is. A
    // reading resampled across the window draws a flat line with a gap after
    // it, which reads as traffic that stopped.
    let collecting = samples == 1;
    let rx_peak = rx.iter().copied().max().unwrap_or(0);
    let tx_peak = tx.iter().copied().max().unwrap_or(0);
    let peak = rx_peak.max(tx_peak).max(1);
    // Split the scales only when the quieter direction would otherwise be a
    // single row, and only when it has something to show.
    let split = rx_peak > 0 && tx_peak > 0 && (rx_peak / tx_peak >= 20 || tx_peak / rx_peak >= 20);

    // The widest label the scale can print: `robust_max` may sit half again
    // above the peak for a steady series, so the gutter is sized for that.
    let top_hint = bps((peak * 3 / 2) as f64);
    let gutter = (top_hint.chars().count() as u16 + 1).clamp(4, 14);
    if inner.width <= gutter + 8 {
        return;
    }
    // The plot is the time window, not the record: it spans the panel, and each
    // sample sits at its age inside the window. The record sets neither the
    // width nor where a tick lands.
    let capacity = (inner.width - gutter) as usize;
    let plot = if collecting {
        Rect {
            x: inner.x + inner.width - 1,
            y: inner.y + 1,
            width: 1,
            height: inner.height - 2,
        }
    } else {
        Rect {
            x: inner.x + gutter,
            y: inner.y + 1,
            width: capacity as u16,
            height: inner.height - 2,
        }
    };

    // The Signal tab gives this panel half a frame: peak and average there
    // would be cut mid-word, so the two live readings keep the row alone.
    let mean = {
        let n = rx.len() + tx.len();
        if n == 0 {
            0
        } else {
            (rx.iter().sum::<u64>() + tx.iter().sum::<u64>()) / n as u64
        }
    };
    let meta = if inner.width >= 56 {
        format!("   peak {} · avg {}", bps(peak as f64), bps(mean as f64))
    } else {
        String::new()
    };
    f.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled("▲ ", Style::default().fg(theme.ok)),
            Span::styled(bps(last(&app.history_dl)), Style::default().fg(theme.ok)),
            Span::styled("  ▼ ", Style::default().fg(theme.accent)),
            Span::styled(
                bps(last(&app.history_ul)),
                Style::default().fg(theme.accent),
            ),
            Span::styled(meta, Style::default().fg(theme.dim)),
        ])),
        Rect { height: 1, ..inner },
    );

    // A single reading keeps no baseline: a floor line under one column would
    // read as a series that went quiet.
    let opts = if collecting {
        crate::ui::chart::ChartOpts {
            baseline: false,
            ..opts
        }
    } else {
        opts
    };
    let (rx_plot, tx_plot) = if collecting {
        (rx.clone(), tx.clone())
    } else {
        (
            crate::ui::chart::resample_to_window(&rx, secs_per_sample, window, capacity),
            crate::ui::chart::resample_to_window(&tx, secs_per_sample, window, capacity),
        )
    };
    // A percentile ceiling, not the peak: one burst stays visible as a clipped
    // tower instead of flattening the body of the series onto the floor. The
    // meta line still reports the true peak and average. Only the slots the
    // record reaches are sampled: the empty left of a short history is not a
    // reading, and counting its zeros would flatten the scale.
    let covered = rx_plot.len().saturating_sub(samples.min(rx_plot.len()));
    let rx_clip = crate::ui::chart::robust_max(&rx_plot[covered..], 0.95);
    let tx_clip = crate::ui::chart::robust_max(&tx_plot[covered..], 0.95);
    let (rx_max, tx_max) = if split {
        (rx_clip, tx_clip)
    } else {
        let shared = rx_clip.max(tx_clip);
        (shared, shared)
    };
    let rx_h = crate::ui::chart::render_mirrored_scaled(
        f.buffer_mut(),
        plot,
        &rx_plot,
        &tx_plot,
        rx_max,
        tx_max,
        app.graph.is_bars(),
        theme.ok,
        theme.accent,
        opts,
    );

    let mut label = |y: u16, text: String| {
        f.render_widget(
            Paragraph::new(Line::from(Span::styled(
                text,
                Style::default().fg(theme.dim),
            ))),
            Rect {
                x: inner.x,
                y,
                width: gutter,
                height: 1,
            },
        );
    };
    label(plot.y, bps(rx_max as f64));
    // The zero row needs a row of its own: with `plot.height == 2` the rx half is
    // one row, and a second label there would overwrite the scale with "0".
    if rx_h > 1 {
        label(plot.y + rx_h - 1, "0".to_string());
    }
    if split {
        label(plot.y + plot.height - 1, bps(tx_max as f64));
    }

    // The axis belongs to the drawn region: a narrow plot cannot hold three
    // ticks, and a status word is not a time span. One sample still says what is
    // happening (the plot is a single column, so the strip spans the panel).
    let axis = if collecting {
        "collecting…".to_string()
    } else {
        let full = crate::ui::chart::time_axis_window(plot.width, window, record_secs);
        if full.chars().count() <= plot.width as usize {
            full
        } else {
            narrow_axis(plot.width, window)
        }
    };
    if !axis.is_empty() {
        let strip = if collecting {
            Rect {
                x: inner.x + gutter,
                width: inner.width - gutter,
                ..plot
            }
        } else {
            plot
        };
        f.render_widget(
            Paragraph::new(Line::from(Span::styled(
                axis,
                Style::default().fg(theme.dim),
            ))),
            Rect {
                x: strip.x,
                y: inner.y + inner.height - 1,
                width: strip.width,
                height: 1,
            },
        );
    }
}

/// The axis for a plot too narrow for three ticks: the window start on the
/// left, `now` on the right, midpoint dropped.
fn narrow_axis(width: u16, secs: u64) -> String {
    let w = width as usize;
    if w < 3 {
        return String::new();
    }
    let left = format!("-{secs}s");
    if w >= left.chars().count() + 4 {
        let pad = w - left.chars().count() - 3;
        format!("{left}{}now", " ".repeat(pad))
    } else {
        "now".to_string()
    }
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

    throughput_panel(f, graph, app);

    // Bottom: the two things a router page never shows side by side - hosts and errors.
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
                    pad(c.ip.as_deref().unwrap_or("-"), 15),
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
                sig.band.clone().unwrap_or_else(|| "-".into()),
                match (sig.dl_mhz, sig.ul_mhz) {
                    (Some(d), Some(u)) => format!("{d}/{u} MHz"),
                    _ => "-".into(),
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
            sig.pci.map(|p| p.to_string()).unwrap_or_else(|| "-".into()),
            sig.earfcn.clone().unwrap_or_else(|| "-".into()),
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
                .unwrap_or_else(|| "-".into()),
            secondary,
            sig.ul_mcs
                .map(|m| m.to_string())
                .unwrap_or_else(|| "-".into()),
            sig.cqi.map(|m| m.to_string()).unwrap_or_else(|| "-".into()),
            app.signal_quality()
        ),
        Style::default().fg(t.fg),
        t.dim,
    ));
    v
}

/// How old a result is, from two unix seconds. Pure, so a test passes `now`.
pub fn age_label(at_unix: u64, now_unix: u64) -> String {
    let secs = now_unix.saturating_sub(at_unix);
    match secs {
        0..=59 => format!("{secs}s ago"),
        60..=3599 => format!("{}m ago", secs / 60),
        _ => format!("{}h ago", secs / 3600),
    }
}

fn speed_lines(app: &App) -> Vec<Line<'static>> {
    let t = &app.theme;
    let Some(s) = &app.snap else {
        return vec![Line::from("waiting…")];
    };
    let tr = &s.traffic;
    let mut v = vec![
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
    ];
    // One line only: the LINK panel is seven rows, and a second line either
    // clips or pushes the whole column taller.
    if let Some(r) = &app.speed {
        v.push(kv(
            "test",
            format!(
                "↓ {} · ↑ {} · {:.1} s · {}",
                bitrate(r.download_bps),
                bitrate(r.upload_bps),
                r.down_secs,
                age_label(r.at_unix, crate::app::unix_now())
            ),
            Style::default().fg(t.fg),
            t.dim,
        ));
    }
    v
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
                nr.band.clone().unwrap_or_else(|| "-".into()),
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
            s.device.wan_ip.clone().unwrap_or_else(|| "-".into()),
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

/// One Bands cell: six columns, so every row of the grid is a matrix whatever
/// the band number's width. The brackets mean "ticked", exactly as before.
fn band_cell(band: u32, app: &App, serving: &str) -> (String, Style) {
    let t = &app.theme;
    let ticked = app.bands_selected.contains(&(band as u8));
    let label = if ticked {
        format!("[B{band}]")
    } else {
        format!("B{band}")
    };
    // 6, not 5: `[B20]` fills five columns, so the sixth is the gap that keeps
    // `[B20]B28` from happening. This is what the alignment test measures.
    let text = format!("{label:<6}");
    let mut style = if ticked {
        Style::default().fg(t.ok).add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(t.dim)
    };
    if format!("B{band}") == serving {
        style = style.add_modifier(Modifier::UNDERLINED);
        if !ticked {
            style = style.fg(t.accent);
        }
    }
    (text, style)
}

/// `B1+B3+B20`
fn plus_list(bands: &[u8]) -> String {
    bands
        .iter()
        .map(|b| format!("B{b}"))
        .collect::<Vec<_>>()
        .join("+")
}

/// A class the device runs today that this tick drops entirely, named with its
/// bands. A class the device already had empty is not news, so it stays quiet.
fn empty_class_warning(app: &App) -> Option<String> {
    let on_device = app.bands_on_device();
    for (cls, members) in app.bands_by_class() {
        if cls == crate::router::bands::BandClass::Other {
            continue;
        }
        let device_uses_it = members.iter().any(|b| on_device.contains(&(*b as u8)));
        let tick_drops_it = members
            .iter()
            .all(|b| !app.bands_selected.contains(&(*b as u8)));
        if device_uses_it && tick_drops_it {
            let names = members
                .iter()
                .map(|b| format!("B{b}"))
                .collect::<Vec<_>>()
                .join(" ");
            return Some(format!(
                "no {} band ticked: none of {} is in the merge",
                cls.label(),
                names
            ));
        }
    }
    None
}

/// The firmware's mode code as the panel spells it, in one place so `now` and
/// `last` cannot drift apart.
fn mode_label(code: &str) -> &str {
    match code {
        "00" => "Auto",
        "03" => "4G only",
        "08" => "5G NR",
        other => other,
    }
}

pub fn bands_tab(f: &mut Frame, area: Rect, app: &App) {
    let t = &app.theme;
    let Some(s) = &app.snap else {
        f.render_widget(
            Paragraph::new("waiting…").block(panel("BANDS".into(), t)),
            area,
        );
        return;
    };

    const LBL: usize = 9; // label column
    const CLS: usize = 6; // class column, so `other` never touches the first cell
    const IND: usize = LBL + CLS; // 15: where the band cells start
    const CELL: usize = 6; // one cell, gap included

    let mode_text = mode_label(&app.band_mode);
    let option_label = if app.band_mode == "08" {
        match app.band_option.as_str() {
            "0" => "NSA",
            "1" => "SA",
            _ => "SA+NSA",
        }
    } else {
        "-"
    };
    let serving_lte = s.signal.band.clone().unwrap_or_else(|| "-".into());
    let serving_text = match s.signal.nr.as_ref().and_then(|n| n.band.clone()) {
        Some(nr) => format!("{serving_lte} LTE + {nr} NR"),
        None => format!("{serving_lte} LTE"),
    };
    let width = area.width.saturating_sub(2) as usize;
    let compact = width < 80; // `now` row: shrink the mode column
    let narrow = width < 70; // action row: drop the explanation
    let order = app.bands_in_draw_order();
    let cursor_band = order.get(app.band_cursor).copied();
    let all_bands = s.all_bands_mask.clone().unwrap_or_else(|| "unknown".into());

    let mut lines = Vec::new();

    // 1. what the link is doing right now.
    // `pad` cuts at the width it is given; 28 is wider than the longest label
    // ("Auto · nr SA+NSA" is 16), so a firmware code we do not know still shows.
    // Below 80 columns the mode column shrinks, otherwise `serving` wraps and
    // leaves "NR" alone on a line of its own.
    lines.push(Line::from(vec![
        Span::styled(pad("now", LBL), Style::default().fg(t.dim)),
        Span::styled(
            pad(
                &format!("{mode_text} · nr {option_label}"),
                if compact { 18 } else { 28 },
            ),
            Style::default().fg(t.accent).add_modifier(Modifier::BOLD),
        ),
        Span::styled(pad("serving", 8), Style::default().fg(t.dim)),
        Span::styled(serving_text, Style::default().fg(t.fg)),
    ]));
    let on_device: Vec<String> = app
        .bands_on_device()
        .iter()
        .map(|b| format!("B{b}"))
        .collect();
    lines.push(Line::from(vec![
        Span::styled(pad("on", LBL), Style::default().fg(t.dim)),
        Span::styled(
            if on_device.is_empty() {
                "unknown".to_string()
            } else {
                on_device.join(" ")
            },
            Style::default().fg(t.fg),
        ),
        Span::styled("   device now", Style::default().fg(t.dim)),
    ]));
    // `all_bands_mask` is what the firmware advertises, not what it runs. Say
    // exactly that, and drop the line on a short terminal where the tick row
    // matters more than the mask (`snapshot --json` still carries it).
    if area.height >= 18 {
        lines.push(Line::from(vec![
            Span::styled(pad("", LBL), Style::default().fg(t.dim)),
            Span::styled(
                format!("advertised {all_bands} · firmware value"),
                Style::default().fg(t.dim),
            ),
        ]));
    }

    // What the link actually is, from fields the snapshot already carries. Two
    // shapes (with and without EARFCN) and a cut as the last resort: this panel
    // wraps, and a wrapped row costs the row below it.
    if area.height >= 20 {
        let mut parts: Vec<String> = Vec::new();
        if serving_lte != "-" {
            parts.push(serving_lte.clone());
        }
        if let Some(mhz) = s.signal.dl_mhz {
            parts.push(format!("{mhz} MHz"));
        }
        if let Some(bw) = s.signal.dl_bw.clone() {
            parts.push(bw);
        }
        if let Some(tm) = s.signal.tm.clone() {
            parts.push(tm);
        }
        if let Some(pci) = s.signal.pci {
            parts.push(format!("PCI {pci}"));
        }
        let core = parts.join(" · ");
        let earfcn = s
            .signal
            .earfcn
            .clone()
            .map(|e| format!("   EARFCN {e}"))
            .unwrap_or_default();
        let with_earfcn = format!("{core}{earfcn}");
        let text = if LBL + with_earfcn.chars().count() <= width {
            with_earfcn
        } else {
            core
        };
        lines.push(Line::from(vec![
            Span::styled(pad("link", LBL), Style::default().fg(t.dim)),
            Span::styled(
                truncate(&text, width.saturating_sub(LBL)),
                Style::default().fg(t.fg),
            ),
        ]));
    }
    if area.height >= 22 {
        if let Some(mcs) = s.signal.dl_mcs_secondary {
            let msg = format!("secondary DL carrier · MCS {mcs}");
            lines.push(Line::from(vec![
                Span::styled(pad("ca", LBL), Style::default().fg(t.dim)),
                Span::styled(
                    truncate(&msg, width.saturating_sub(LBL)),
                    Style::default().fg(t.fg),
                ),
            ]));
        }
    }
    lines.push(Line::from(""));

    // 2. the grid, grouped so a class can be judged as a class.
    let per_row = (width.saturating_sub(IND) / CELL).max(1);
    for (cls, members) in app.bands_by_class() {
        for (row_no, chunk) in members.chunks(per_row).enumerate() {
            let mut spans = vec![
                Span::styled(
                    pad(if row_no == 0 { "bands" } else { "" }, LBL),
                    Style::default().fg(t.dim),
                ),
                Span::styled(pad(cls.label(), IND - LBL), Style::default().fg(t.dim)),
            ];
            for band in chunk {
                let (text, style) = if Some(*band) == cursor_band {
                    (
                        band_cell(*band, app, &serving_lte).0,
                        Style::default()
                            .fg(active_text_colour(t))
                            .bg(t.accent)
                            .add_modifier(Modifier::BOLD),
                    )
                } else {
                    band_cell(*band, app, &serving_lte)
                };
                spans.push(Span::styled(text, style));
            }
            lines.push(Line::from(spans));
        }
    }

    // 3. the tick versus the device: one row that reads the change. Three
    // shapes, widest first, so the row never wraps: the band list and the mask
    // drop before the sentence does, because the grid above already shows both
    // and the sentence is the thing the user came for.
    let (added, dropped) = app.bands_diff();
    let counted = format!("{} of {}", app.bands_selected.len(), order.len());
    let listed = format!("{counted} · {}", plus_list(&app.bands_selected));
    let masked = format!("→ 0x{}", app.band_mask_hex());
    let (note, note_style) = if added.is_empty() && dropped.is_empty() {
        (
            "= device, nothing to apply".to_string(),
            Style::default().fg(t.dim),
        )
    } else {
        let n = added.len() + dropped.len();
        // Every dropped band carries its own minus: "-B5+B8" reads as an
        // addition of B8, which is the opposite of what happened.
        let mut parts = Vec::new();
        if !added.is_empty() {
            parts.push(format!("+{}", plus_list(&added)));
        }
        if !dropped.is_empty() {
            parts.push(
                dropped
                    .iter()
                    .map(|b| format!("-B{b}"))
                    .collect::<Vec<_>>()
                    .join(" "),
            );
        }
        (
            format!(
                "{} · {n} {} to apply",
                parts.join(" "),
                if n == 1 { "change" } else { "changes" }
            ),
            Style::default().fg(t.warn),
        )
    };
    let shape = |list: bool, mask: bool| -> String {
        let mut s = if list {
            listed.clone()
        } else {
            counted.clone()
        };
        if mask {
            s.push_str("  ");
            s.push_str(&masked);
        }
        s.push_str("  ");
        s.push_str(&note);
        s
    };
    let fits = |list: bool, mask: bool| LBL + shape(list, mask).chars().count() <= width;
    let (list, mask) = if fits(true, true) {
        (true, true)
    } else if fits(true, false) {
        (true, false)
    } else {
        (false, false)
    };
    let mut tick_spans = vec![Span::styled(pad("ticked", LBL), Style::default().fg(t.dim))];
    tick_spans.push(Span::styled(
        if list { listed } else { counted },
        Style::default().fg(t.fg).add_modifier(Modifier::BOLD),
    ));
    if mask {
        tick_spans.push(Span::styled(
            format!("  {masked}"),
            Style::default().fg(t.fg),
        ));
    }
    // Last resort: the note itself is cut rather than wrapped, so the panel
    // keeps its shape even on a list of forty bands.
    let room = width.saturating_sub(LBL + if mask { masked.chars().count() + 2 } else { 0 } + 2);
    tick_spans.push(Span::styled(
        format!("  {}", truncate(&note, room)),
        note_style,
    ));
    lines.push(Line::from(tick_spans));

    // A tick set that drops the band carrying the link is the fact worth naming,
    // so it sits right under the verdict. The text claims no outcome: on 4G the
    // radio may re-attach elsewhere, and saying "drops the link" would be a
    // promise the firmware never made.
    let device_mask_known = s.net_mode.lte_band.is_some();
    if device_mask_known && !app.bands_selected.is_empty() && serving_lte != "-" {
        if let Some(band) = serving_lte
            .strip_prefix('B')
            .and_then(|n| n.parse::<u8>().ok())
        {
            if !app.bands_selected.contains(&band) {
                let msg = format!("B{band} carries the link and is not in the tick set");
                lines.push(Line::from(vec![
                    Span::styled(pad("gap", LBL), Style::default().fg(t.warn)),
                    Span::styled(
                        truncate(&msg, width.saturating_sub(LBL)),
                        Style::default().fg(t.warn),
                    ),
                ]));
            }
        }
    }

    // A whole class going dark is the mistake worth naming out loud.
    if let Some(warning) = empty_class_warning(app) {
        lines.push(Line::from(vec![
            Span::styled(pad("warn", LBL), Style::default().fg(t.warn)),
            Span::styled(warning, Style::default().fg(t.warn)),
        ]));
    }
    lines.push(Line::from(""));

    // 4. presets, numbered so `n` says where it is.
    if !app.band_presets.is_empty() {
        let mut spans = vec![Span::styled(
            pad("presets", LBL),
            Style::default().fg(t.dim),
        )];
        let mut used = LBL;
        for (i, preset) in app.band_presets.iter().enumerate() {
            let missing = app.preset_missing(i);
            let text = format!(
                "{}:{}{}",
                i + 1,
                preset.name,
                if missing.is_empty() { "" } else { "!" }
            );
            let w = text.chars().count() + 2;
            if used + w > width && spans.len() > 1 {
                lines.push(Line::from(std::mem::take(&mut spans)));
                spans.push(Span::styled(pad("", LBL), Style::default().fg(t.dim)));
                used = LBL;
            }
            let style = if app.preset_cursor == Some(i) {
                Style::default()
                    .fg(active_text_colour(t))
                    .bg(t.accent)
                    .add_modifier(Modifier::BOLD)
            } else if missing.is_empty() {
                Style::default().fg(t.accent)
            } else {
                Style::default().fg(t.warn)
            };
            spans.push(Span::styled(text, style));
            spans.push(Span::raw("  "));
            used += w;
        }
        lines.push(Line::from(spans));
    }
    lines.push(Line::from(""));

    // The way back from a bad apply, one key away and never a write by itself.
    if area.height >= 18 {
        if let Some(mask) = &app.undo_mask {
            if !mask.is_empty() {
                let msg = format!(
                    "back to 0x{} ({} bands) · u",
                    crate::router::bands::to_hex(crate::router::bands::mask_from_bands(mask)),
                    mask.len()
                );
                lines.push(Line::from(vec![
                    Span::styled(pad("undo", LBL), Style::default().fg(t.dim)),
                    Span::styled(
                        truncate(&msg, width.saturating_sub(LBL)),
                        Style::default().fg(t.fg),
                    ),
                ]));
            }
        }
    }

    // The last apply and the NR side of the link: history and the other radio,
    // both only on a tall enough panel.
    if area.height >= 24 {
        if let Some(last) = &app.last_apply {
            let msg = format!(
                "applied {} ago · mode {} · 0x{} · {}",
                dur(last.at.elapsed().as_secs().max(1)),
                mode_label(&last.mode),
                crate::router::bands::to_hex(crate::router::bands::mask_from_bands(&last.bands)),
                last.verdict
            );
            lines.push(Line::from(vec![
                Span::styled(pad("last", LBL), Style::default().fg(t.dim)),
                Span::styled(
                    truncate(&msg, width.saturating_sub(LBL)),
                    Style::default().fg(t.fg),
                ),
            ]));
        }
        let text = match s.signal.nr.as_ref() {
            Some(n) => format!(
                "{} serving · bars {}/5 · ENDC {}",
                n.band.clone().unwrap_or_else(|| "-".into()),
                s.connection.nr_bars,
                if s.connection.endc { "on" } else { "off" }
            ),
            None => "no NR band reported by this firmware".to_string(),
        };
        lines.push(Line::from(vec![
            Span::styled(pad("nr", LBL), Style::default().fg(t.dim)),
            Span::styled(
                truncate(&text, width.saturating_sub(LBL)),
                Style::default().fg(t.fg),
            ),
        ]));
    }

    // 5. the action, its cost and its outcome, on one always-visible row: the
    // offline warning must survive 60x20, where the legend is dropped.
    let write_style = match &app.write {
        crate::write::WriteState::Failed { .. } => Style::default().fg(t.danger),
        crate::write::WriteState::Success { .. } => Style::default().fg(t.ok),
        crate::write::WriteState::Idle => Style::default().fg(t.dim),
        _ => Style::default().fg(t.warn),
    };
    let action_text = if app.bands_unchanged() {
        if narrow {
            "a re-apply"
        } else {
            "a re-apply, forces a re-attach"
        }
    } else if narrow {
        "a apply (y)"
    } else {
        "a apply (y confirms)"
    };
    let offline = " · 10-30 s offline";
    let write_prefix = "   write ";
    // The verdict can be long ("applied · router answered 100006 …"), and this
    // row must stay one line: it is the row the panel promises to keep.
    let room = width.saturating_sub(
        LBL + action_text.chars().count() + offline.chars().count() + write_prefix.chars().count(),
    );
    lines.push(Line::from(vec![
        Span::styled(pad("action", LBL), Style::default().fg(t.dim)),
        Span::styled(action_text, Style::default().fg(t.fg)),
        Span::styled(offline, Style::default().fg(t.warn)),
        Span::styled(write_prefix, Style::default().fg(t.dim)),
        Span::styled(truncate(&app.write.label(), room.max(4)), write_style),
    ]));

    // The panel carries its own keys, `a apply` included: the status bar clips
    // them below 100 columns and shows nothing at all at 60.
    if area.height >= 20 {
        lines.push(Line::from(Span::styled(
            // Measured, not guessed: 69 columns fits 80, 56 fits 60. The
            // earlier pair drifted and the wide one wrapped at 100.
            if width >= 75 {
                "keys  space tick · j/k · a apply · u undo · n preset · s save · x del"
            } else {
                "keys  space · a apply · u undo · s save"
            },
            Style::default().fg(t.dim),
        )));
    }
    if area.height >= 22 {
        lines.push(Line::from(Span::styled(
            "! = band this device does not list, preset only",
            Style::default().fg(t.dim),
        )));
    }

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
    // The table gets a panel of its own; below 24 rows the stats panel keeps
    // every fact and names the missing table instead of clipping a row.
    let tall = area.height >= 24;
    let (top, usage_area, dns_area) = if tall {
        let [a, b, c] = Layout::vertical([
            // Nine would leave seven inner rows for eight lines and clip the
            // reset row the test pins; ten gives it eight.
            Constraint::Length(10),
            Constraint::Min(7),
            Constraint::Length(7),
        ])
        .areas(area);
        (a, Some(b), c)
    } else {
        let [a, c] = Layout::vertical([Constraint::Min(8), Constraint::Length(7)]).areas(area);
        (a, None, c)
    };
    let Some(s) = &app.snap else {
        f.render_widget(
            Paragraph::new("waiting…").block(panel("TRAFFIC".into(), t)),
            top,
        );
        return;
    };
    let tr = &s.traffic;
    let used = crate::usage::used(tr, s.quota.as_ref(), &app.usage);
    let used_gb = used as f64 / 1024.0 / 1024.0 / 1024.0;
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
    // Which number the bar is measuring, in the panel, not in a README.
    let source = if s.quota.is_some() {
        "router month counter"
    } else {
        "local ledger since the baseline"
    };
    let mut lines = vec![
        Line::from(vec![
            Span::styled("down ", Style::default().fg(t.dim)),
            Span::styled(
                bps(tr.dl_rate_bytes as f64),
                Style::default().fg(t.ok).add_modifier(Modifier::BOLD),
            ),
            Span::raw("      "),
            Span::styled("up ", Style::default().fg(t.dim)),
            Span::styled(
                bps(tr.ul_rate_bytes as f64),
                Style::default().fg(t.accent).add_modifier(Modifier::BOLD),
            ),
        ]),
        Line::from(""),
        kv(
            "session",
            format!(
                "{} ↓ / {} ↑ · {}",
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
                "{} ↓ / {} ↑ · {}",
                bytes(tr.total_dl_bytes),
                bytes(tr.total_ul_bytes),
                dur(tr.total_secs)
            ),
            Style::default().fg(t.fg),
            t.dim,
        ),
        Line::from(""),
        kv(
            "budget",
            format!("{used_gb:.1} GB of {:.0} GB · {source}", app.budget_gb),
            Style::default().fg(colour),
            t.dim,
        ),
        bar(frac, (top.width.saturating_sub(24)) as usize, colour, t.dim),
        Line::from(Span::styled(
            // At zero the baseline is set by the first sample; printing a date
            // there would say 1970.
            if app.usage.reset_at == 0 {
                "baseline starts with the first sample · b resets the baseline".to_string()
            } else {
                format!(
                    "reset {} · b resets the baseline · auto at cycle start",
                    crate::usage::date_label(app.usage.reset_at)
                )
            },
            Style::default().fg(t.dim),
        )),
    ];
    if !tall {
        lines.push(Line::from(Span::styled(
            "usage table needs 24 rows",
            Style::default().fg(t.dim),
        )));
        lines.extend(crate::ui::chart::spark_auto(
            &app.history_dl,
            top.width.saturating_sub(4) as usize,
            2,
            app.graph.is_bars(),
            t.ok,
            app.chart_opts(),
        ));
    }
    f.render_widget(
        Paragraph::new(lines)
            .wrap(Wrap { trim: false })
            .block(panel("TRAFFIC".into(), t)),
        top,
    );
    if let Some(area) = usage_area {
        usage_panel(f, area, app, s);
    }

    /// Today, this week and this month, from the ledger. The month row prefers the
    /// router's own counter; the ledger only fills the gap when the firmware
    /// refuses it, and the title says which one is on screen.
    fn usage_panel(f: &mut Frame, area: Rect, app: &App, s: &crate::model::Snapshot) {
        let t = &app.theme;
        let inner = area.height.saturating_sub(2);
        if inner < 5 {
            return;
        }
        let today = crate::usage::local_day(s.generated_at);
        let tot = crate::usage::totals(&app.usage, today);
        let q = s.quota.as_ref();
        let title = if q.is_some() {
            "USAGE · month row = router counter"
        } else {
            "USAGE · local ledger"
        };
        let row = |label: &str, dl: u64, ul: u64, style: Style| {
            let total = dl.saturating_add(ul);
            Line::from(Span::styled(
                format!(
                    "{label:<8}{:>11}{:>11}{:>11}",
                    bytes(dl),
                    bytes(ul),
                    bytes(total)
                ),
                style,
            ))
        };
        let (mdl, mul) = match q {
            Some(q) => (q.month_dl_bytes, q.month_ul_bytes),
            None => tot.month,
        };
        let mut lines = vec![
            Line::from(Span::styled(
                format!("{:<8}{:>11}{:>11}{:>11}", "period", "down", "up", "total"),
                Style::default().fg(t.dim),
            )),
            row("today", tot.today.0, tot.today.1, Style::default().fg(t.fg)),
            row("week", tot.week.0, tot.week.1, Style::default().fg(t.fg)),
            row(
                if q.is_some() { "month*" } else { "month" },
                mdl,
                mul,
                Style::default().fg(if q.is_some() { t.accent } else { t.fg }),
            ),
        ];
        if inner >= 6 {
            let covered = tot
                .first_day
                .filter(|f| *f <= today.num)
                .map(|f| today.num - f + 1)
                .unwrap_or(0);
            lines.push(Line::from(Span::styled(
                format!(
                    "ledger covers {covered} day(s) · week starts Sunday{}",
                    if q.is_some() {
                        " · month* = router cycle"
                    } else {
                        ""
                    }
                ),
                Style::default().fg(t.dim),
            )));
        }
        // Whatever rows are left go to the recent days, newest first: the rows
        // above answer "how much", this list answers "when".
        let free = (inner as usize).saturating_sub(lines.len());
        if free > 0 {
            for b in app.usage.days.iter().rev().take(free) {
                let (_, m, d) = crate::usage::civil_from_days(b.day);
                lines.push(row(
                    &format!("{m:02}-{d:02}"),
                    b.dl,
                    b.ul,
                    Style::default().fg(t.dim),
                ));
            }
        }
        f.render_widget(
            Paragraph::new(lines).block(panel(title.to_string(), t)),
            area,
        );
    }

    // DNS panel - the write target of this tab.
    let mut dns_lines = Vec::new();
    match &s.dns {
        Some(d) => {
            let manual = d.is_manual();
            let mode = if manual {
                "manual".to_string()
            } else {
                match &d.lan_ip {
                    Some(lan) => format!("router as resolver ({lan})"),
                    None => "router as resolver".to_string(),
                }
            };
            dns_lines.push(kv(
                "status",
                mode,
                Style::default().fg(if manual { t.accent } else { t.fg }),
                t.dim,
            ));
            dns_lines.push(kv(
                "primary",
                d.primary.clone().unwrap_or_else(|| "-".into()),
                Style::default().fg(t.fg),
                t.dim,
            ));
            dns_lines.push(kv(
                "secondary",
                d.secondary.clone().unwrap_or_else(|| "-".into()),
                Style::default().fg(t.fg),
                t.dim,
            ));
            if d.editable == Some(false) {
                dns_lines.push(kv(
                    "editable",
                    "no: this firmware has no LAN DHCP DNS setting".to_string(),
                    Style::default().fg(t.warn),
                    t.dim,
                ));
            }
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

/// Row budget for the bottom strip. One function so the title, the test and the
/// drawing cannot disagree.
pub(crate) fn split(height: u16) -> (u16, u16, u16) {
    // 15 = link 8 + the smallest usable ping block 7.
    let hist = height.saturating_sub(15).clamp(0, 14);
    let mut link = 8;
    let ping = height.saturating_sub(hist + link).clamp(7, 11);
    // Surplus past the ping panel's comfortable size goes to CELL | LINK, which
    // simply gains blank lines inside its border.
    link += height.saturating_sub(hist + link + ping);
    (link, ping, hist)
}

/// Rows of saved runs the panel can draw in `height` rows: the border takes two,
/// and nothing else is reserved, so no space is spent on a footer that would
/// steal a data row when the panel grows.
pub(crate) fn visible_rows(height: u16) -> usize {
    (height.saturating_sub(2) as usize).min(crate::history::SHOWN)
}

pub fn network_tab(f: &mut Frame, area: Rect, app: &App) {
    let t = &app.theme;
    if area.height < 18 {
        // Cramped terminal: the ping panel alone, no saved runs.
        let panel_rows = if area.height >= 12 { 7 } else { 5 };
        let [top, ping_area] =
            Layout::vertical([Constraint::Min(4), Constraint::Length(panel_rows)]).areas(area);
        // Drawn before the `snap` guard: a finished test is the user's, not the
        // router's, and must survive a poll that failed.
        ping_speed_panel(f, ping_area, app);
        draw_cell_link(f, top, app, t);
        return;
    }
    let (link_h, ping_h, hist_h) = split(area.height);
    let [top, ping_area, hist_area] = Layout::vertical([
        Constraint::Length(link_h),
        Constraint::Length(ping_h),
        Constraint::Length(hist_h),
    ])
    .areas(area);
    ping_speed_panel(f, ping_area, app);
    speed_history_panel(f, hist_area, app);
    draw_cell_link(f, top, app, t);
}

/// CELL and LINK, one block of radio facts and one of the link facts.
fn draw_cell_link(f: &mut Frame, top: Rect, app: &App, t: &crate::theme::Theme) {
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
            sig.tac.map(|v| v.to_string()).unwrap_or_else(|| "-".into()),
            Style::default().fg(t.fg),
            t.dim,
        ),
        kv(
            "pci",
            sig.pci.map(|v| v.to_string()).unwrap_or_else(|| "-".into()),
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
}

/// The ping block and the speed test share one panel: both measure the path
/// through this machine, and a reader needs them side by side. The numbers go
/// down first; the explaining lines fill whatever rows remain, in a fixed
/// priority, so a short panel hides prose but never a measurement.
fn ping_speed_panel(f: &mut Frame, area: Rect, app: &App) {
    let t = &app.theme;
    let room = area.height.saturating_sub(2) as usize;
    // Rank 0 is a number or a live status and is never dropped. Ranks 1 to 3
    // are dropped from the bottom of that order when the panel is short: how
    // to run it, what it ran over, then the result note.
    let mut rows: Vec<(u8, Line)> = Vec::new();
    match &app.ping {
        Some(p) => {
            rows.push((
                0,
                Line::from(vec![
                    Span::styled(format!("{:<16}", p.host), Style::default().fg(t.fg)),
                    Span::styled(
                        p.detail.clone(),
                        Style::default().fg(if p.ok { t.ok } else { t.danger }),
                    ),
                ]),
            ));
            if let Some(avg) = p.avg_ms {
                rows.push((
                    0,
                    Line::from(Span::styled(
                        format!("rtt avg {avg:.1} ms · packets {}/{}", p.received, p.sent),
                        Style::default().fg(t.dim),
                    )),
                ));
            }
        }
        None => rows.push((
            0,
            Line::from(Span::styled("no test yet", Style::default().fg(t.dim))),
        )),
    }
    match (&app.speed_running, &app.speed_progress) {
        (true, Some(p)) => {
            let phase = match p.phase {
                crate::speedtest::Phase::Download => "downloading",
                crate::speedtest::Phase::Upload => "uploading",
            };
            rows.push((
                0,
                Line::from(Span::styled(
                    format!("{phase} · {} · {}", bytes(p.bytes), bitrate(p.rate_bps)),
                    Style::default().fg(t.accent),
                )),
            ));
            // While the link is saturated the router polls may time out; say it
            // during the run instead of letting a stale header look broken.
            rows.push((
                0,
                Line::from(Span::styled(
                    "router polls may fail while the link is busy",
                    Style::default().fg(t.dim),
                )),
            ));
        }
        _ => {
            if let Some(r) = &app.speed {
                let mut spans = vec![
                    Span::styled("test ", Style::default().fg(t.dim)),
                    Span::styled(
                        format!(
                            "↓ {} · ↑ {}",
                            bitrate(r.download_bps),
                            bitrate(r.upload_bps)
                        ),
                        Style::default().fg(t.ok).add_modifier(Modifier::BOLD),
                    ),
                    Span::styled(
                        format!(
                            " · {:.1} s · {} down · ttfb {:.0} ms · {}",
                            r.down_secs,
                            bytes(r.down_bytes),
                            r.ttfb_ms,
                            age_label(r.at_unix, crate::app::unix_now())
                        ),
                        Style::default().fg(t.dim),
                    ),
                ];
                if r.short_window {
                    spans.push(Span::styled(
                        " · ramp-limited estimate",
                        Style::default().fg(t.warn),
                    ));
                }
                if r.partial {
                    spans.push(Span::styled(" · partial", Style::default().fg(t.warn)));
                }
                rows.push((0, Line::from(spans)));
                if let Some(note) = &r.note {
                    rows.push((
                        3,
                        Line::from(Span::styled(note.clone(), Style::default().fg(t.warn))),
                    ));
                }
                if let Some(c) = &app.speed_context {
                    // A test can cross a band change on a 5G CPE; both ends are
                    // shown when they differ, never just the last one.
                    let band = match (&c.band_at_start, &c.band) {
                        (Some(a), b) if a != b => format!("{a} → {b}"),
                        _ => c.band.clone(),
                    };
                    rows.push((
                        2,
                        Line::from(Span::styled(
                            format!(
                                "through this host · {} · {} · peak device ↓ {} ↑ {}",
                                c.net_type,
                                band,
                                // Byte counters, so `bps`, not `bitrate`.
                                bps(c.router_peak_dl as f64),
                                bps(c.router_peak_ul as f64)
                            ),
                            Style::default().fg(t.dim),
                        )),
                    ));
                }
            } else {
                rows.push((
                    0,
                    Line::from(Span::styled(
                        "no speed test yet · T on this tab measures the path through this host",
                        Style::default().fg(t.dim),
                    )),
                ));
            }
        }
    }
    if let Some(e) = &app.speed_error {
        rows.push((
            0,
            Line::from(Span::styled(
                format!("speed test: {e}"),
                Style::default().fg(t.danger),
            )),
        ));
    }
    rows.push((
        1,
        Line::from(Span::styled(
            "p ping · T speed test (both from THIS machine, not from the router)",
            Style::default().fg(t.dim),
        )),
    ));

    // Admit optional rows by rank until the panel is full, then draw the
    // survivors in reading order.
    let mut admit: Vec<u8> = Vec::new();
    let mut used = rows.iter().filter(|(rank, _)| *rank == 0).count();
    for rank in 1..=3u8 {
        let count = rows.iter().filter(|(r, _)| *r == rank).count();
        if count > 0 && used + count <= room {
            used += count;
            admit.push(rank);
        }
    }
    let shown: Vec<Line> = rows
        .into_iter()
        .filter(|(rank, _)| *rank == 0 || admit.contains(rank))
        .map(|(_, line)| line)
        .collect();
    f.render_widget(
        Paragraph::new(shown).block(panel("PING · SPEED".into(), t)),
        area,
    );
}

/// The last `history::SHOWN` runs, newest first. The title carries the count of
/// rows actually drawn and the legend, so the rows are all data.
fn speed_history_panel(f: &mut Frame, area: Rect, app: &App) {
    let t = &app.theme;
    let rows = visible_rows(area.height);
    let shown = app.history.len().min(rows);
    let title = if app.history_total == 0 {
        "SPEED · no saved runs".to_string()
    } else if app.history_warn.is_some() {
        format!(
            "SPEED · {shown} of {} · history problem, see log",
            app.history_total
        )
    } else {
        // The median is over the rows on screen, so the number beside it is
        // one a reader can check against the lines below.
        let median = crate::history::median_down(&app.history[..shown]);
        format!(
            "SPEED · {shown} of {} · med {:.1} Mbps · ms idle/loaded, * partial, ~band",
            app.history_total,
            median / 1e6
        )
    };
    let block = panel(title, t);
    let inner = block.inner(area);
    f.render_widget(block, area);
    if inner.width == 0 || inner.height == 0 {
        return;
    }
    let now = crate::app::unix_now();
    let mut lines: Vec<Line> = Vec::new();
    for e in app.history.iter().take(rows) {
        let age = dur(now.saturating_sub(e.ended_unix));
        let idle = e
            .ping_idle_ms
            .map(|v| format!("{v:.0}"))
            .unwrap_or_else(|| "-".into());
        let loaded = e
            .ping_loaded_ms
            .map(|v| format!("{v:.0}"))
            .unwrap_or_else(|| "-".into());
        let mut spans = vec![
            Span::styled(format!("{age:<4} "), Style::default().fg(t.dim)),
            Span::styled(
                format!("{:>6.1}↓", e.download_bps / 1e6),
                Style::default().fg(t.ok),
            ),
            Span::styled(
                format!("{:>6.1}↑", e.upload_bps / 1e6),
                Style::default().fg(t.accent),
            ),
            Span::styled(format!("{idle:>4} {loaded:>4}"), Style::default().fg(t.fg)),
        ];
        if e.status == crate::history::Status::Partial {
            spans.push(Span::styled(" *", Style::default().fg(t.warn)));
        }
        if e.band_changed {
            spans.push(Span::styled(" ~band", Style::default().fg(t.warn)));
        }
        lines.push(Line::from(spans));
    }
    if lines.is_empty() {
        lines.push(Line::from(Span::styled(
            "no saved runs · T measures one",
            Style::default().fg(t.dim),
        )));
    }
    f.render_widget(Paragraph::new(lines), inner);
}

// ----------------------------------------------------------------- clients

pub fn clients_tab(f: &mut Frame, area: Rect, app: &App) {
    let t = &app.theme;
    let Some(snap) = &app.snap else {
        f.render_widget(
            Paragraph::new("waiting…").block(panel("CLIENTS".into(), t)),
            area,
        );
        return;
    };
    // A firmware without `wlan/host-list` is not a network with no devices. Say
    // which one it is instead of drawing an empty table under a "0" title.
    if snap.unsupported.iter().any(|id| id == "clients") {
        f.render_widget(
            Paragraph::new(Line::from(Span::styled(
                "not exposed by this firmware",
                Style::default().fg(t.warn),
            )))
            .block(panel("CLIENTS · not exposed".into(), t)),
            area,
        );
        return;
    }
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
            Style::default()
                .bg(t.selected)
                .fg(t.fg)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(t.fg)
        };
        lines.push(Line::from(Span::styled(
            format!(
                "{} {} {} {} {} {}",
                if selected { "▶" } else { " " },
                pad(c.name.as_deref().unwrap_or("(unnamed)"), 26),
                pad(c.ip.as_deref().unwrap_or("-"), 15),
                pad(c.mac.as_deref().unwrap_or("-"), 18),
                pad(c.freq.as_deref().unwrap_or("-"), 6),
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
                    "sms/sms-list as a malformed request (error 100005).",
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
                            pad(m.phone.as_deref().unwrap_or("-"), 16),
                            Style::default().fg(t.fg),
                        ),
                        Span::styled(
                            pad(m.date.as_deref().unwrap_or("-"), 18),
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
            let manual = d.is_manual();
            let mode = if manual {
                "manual".to_string()
            } else {
                match &d.lan_ip {
                    Some(lan) => format!("router as resolver ({lan})"),
                    None => "router as resolver".to_string(),
                }
            };
            lines.push(Line::from(vec![
                Span::styled(pad("current", 10), Style::default().fg(t.dim)),
                Span::styled(
                    pad(&mode, 32),
                    Style::default().fg(if manual { t.accent } else { t.ok }),
                ),
                Span::styled(
                    format!(
                        "{} / {}",
                        d.primary.clone().unwrap_or_else(|| "-".into()),
                        d.secondary.clone().unwrap_or_else(|| "-".into())
                    ),
                    Style::default().fg(t.fg),
                ),
            ]));
            if d.editable == Some(false) {
                lines.push(Line::from(vec![
                    Span::styled(pad("editable", 10), Style::default().fg(t.dim)),
                    Span::styled(
                        "no: this firmware has no LAN DHCP DNS setting",
                        Style::default().fg(t.warn),
                    ),
                ]));
            }
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
    // The box, not the terminal, decides this row: the refusal reason is longer
    // than the panel at 80 columns and ratatui cut it dead at the border, mid
    // word, with no sign that anything was missing. Cut it ourselves, one column
    // short of the frame, so the ellipsis lands inside the box.
    let inner_w = area.width.saturating_sub(2) as usize;
    let room = inner_w.saturating_sub(10);
    let verdict = if room == 0 {
        String::new()
    } else {
        truncate(&app.write.label(), room)
    };
    lines.push(Line::from(vec![
        Span::styled(pad("write", 10), Style::default().fg(t.dim)),
        Span::styled(verdict, write_style),
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
            d.uptime_secs.map(dur).unwrap_or_else(|| "-".into()),
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
                        pin.sim_state.clone().unwrap_or_else(|| "-".into()),
                        pin.pin_state.clone().unwrap_or_else(|| "-".into()),
                        pin.pin_retries.clone().unwrap_or_else(|| "-".into()),
                        pin.puk_retries.clone().unwrap_or_else(|| "-".into())
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
            Paragraph::new("terminal too small for gauges - press V"),
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
        crate::app::SignalPanel::NrBars => "SIGNAL · 5G NR (bars)",
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
            "NR is attached: the firmware sends the nr fields empty here",
            Style::default().fg(t.warn),
        )));
        // Fields this firmware did send are shown, not dropped: an empty RSRP
        // does not make N-ARFCN or the bandwidth a lie. The lead-in rides on
        // the first line to keep the panel short in a compact layout.
        if let Some(nr) = s.signal.nr.as_ref() {
            for l in nr_aux_lines_head(nr, inner.width.saturating_sub(3) as usize, "still reports")
            {
                lines.push(Line::from(Span::styled(l, Style::default().fg(t.dim))));
            }
        }
        // The upgrade hint is the least important line: compact layouts drop it.
        if wide {
            lines.push(Line::from(Span::styled(
                "full gauges appear once nrrsrp, nrrsrq or nrsinr carry a value",
                Style::default().fg(t.dim),
            )));
        }
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
            "RSSI" => m.value.map(|v| t.rssi_color(v)).unwrap_or(t.dim),
            _ => m.value.map(|v| t.rsrp_color(v)).unwrap_or(t.dim),
        };
        let qual = quality_label(m.label, m.value);
        let mut spans = vec![
            Span::styled(pad(m.label, label_w), Style::default().fg(t.dim)),
            Span::styled(
                pad(
                    &m.value
                        .map(|v| format!("{v:.0} {}", m.unit))
                        .unwrap_or_else(|| "-".to_string()),
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
            nr.band.clone().unwrap_or_else(|| "-".into()),
            nr.dl_mhz
                .map(|v| v.to_string())
                .unwrap_or_else(|| "-".into()),
            nr.dl_mcs
                .map(|v| v.to_string())
                .unwrap_or_else(|| "-".into()),
            s.signal
                .pci
                .map(|p| p.to_string())
                .unwrap_or_else(|| "-".into()),
        )
    } else {
        format!(
            " · {} · {} · {} MHz · {} · PCI {} · CQI {}",
            s.connection.net_type,
            s.signal.band.clone().unwrap_or_else(|| "-".into()),
            s.signal
                .dl_mhz
                .map(|v| v.to_string())
                .unwrap_or_else(|| "-".into()),
            s.signal.tm.clone().unwrap_or_else(|| "-".into()),
            s.signal
                .pci
                .map(|p| p.to_string())
                .unwrap_or_else(|| "-".into()),
            s.signal
                .cqi
                .map(|v| v.to_string())
                .unwrap_or_else(|| "-".into()),
        )
    };
    facts.push(Span::styled(detail, Style::default().fg(t.dim)));
    lines.push(Line::from(facts));
    // Secondary NR facts on their own line: they do not fit the primary one
    // and, on this firmware, they are often all that arrives.
    if fiveg {
        for l in nr_aux_lines(&nr, inner.width as usize) {
            lines.push(Line::from(Span::styled(l, Style::default().fg(t.dim))));
        }
    }

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

/// The NR fields that are not main gauge metrics, as separate parts.
fn nr_aux_parts(nr: &crate::model::Nr) -> Vec<String> {
    let mut parts: Vec<String> = Vec::new();
    if let Some(a) = nr.earfcn.as_deref() {
        parts.push(format!("ARFCN {a}"));
    }
    if let Some(bw) = nr.dl_bw.as_deref() {
        parts.push(format!("DL {bw}"));
    }
    if let Some(bw) = nr.ul_bw.as_deref() {
        parts.push(format!("UL {bw}"));
    }
    if let Some(r) = nr.rank {
        parts.push(format!("rank {r}"));
    }
    if let Some(b) = nr.bler_pct {
        parts.push(format!("BLER {b:.1}%"));
    }
    if let Some(c) = nr.cqi {
        parts.push(format!("CQI {c}"));
    }
    if let Some(tx) = nr.txpower_dbm {
        parts.push(format!("TX {tx:.0} dBm"));
    }
    parts
}

/// Packs the parts into lines that fit `width` columns, so nothing is clipped
/// by a non-wrapping paragraph. Empty when the firmware sent none of them.
pub(crate) fn nr_aux_lines(nr: &crate::model::Nr, width: usize) -> Vec<String> {
    nr_aux_lines_head(nr, width, "")
}

/// Same, with a lead-in word kept on the first line when it fits.
pub(crate) fn nr_aux_lines_head(nr: &crate::model::Nr, width: usize, head: &str) -> Vec<String> {
    let width = width.max(12);
    let parts = nr_aux_parts(nr);
    // A lead-in word with nothing after it is noise: return empty so callers
    // draw no line at all.
    if parts.is_empty() {
        return Vec::new();
    }
    let mut out: Vec<String> = Vec::new();
    let mut cur = head.to_string();
    let mut fresh_head = !head.is_empty();
    for part in parts {
        let candidate = if cur.is_empty() {
            part.clone()
        } else if fresh_head {
            fresh_head = false;
            format!("{cur} {part}")
        } else {
            format!("{cur} · {part}")
        };
        if !cur.is_empty() && candidate.chars().count() > width {
            out.push(std::mem::take(&mut cur));
            cur = part;
            fresh_head = false;
        } else {
            cur = candidate;
        }
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
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
    let mut title = match app.history_dl.len().max(app.history_ul.len()) {
        0 => "SPEED · waiting".to_string(),
        1 => "SPEED · 1 sample, collecting".to_string(),
        n => format!(
            "SPEED · last {}s",
            (n as u64 * app.refresh_ms.max(1) / 1000).max(1)
        ),
    };
    // A narrow panel clips the long title mid-word; the readings below already
    // say what the panel is.
    if area.width < 34 {
        title = "SPEED".to_string();
    }
    let block = panel(title, t);
    let inner = block.inner(area);
    f.render_widget(block, area);

    let samples = app.history_dl.len().max(app.history_ul.len());
    if samples == 0 {
        f.render_widget(
            Paragraph::new(Line::from(Span::styled(
                "waiting for the first sample",
                Style::default().fg(t.dim),
            ))),
            inner,
        );
        return;
    }

    // A mirrored plot needs four rows. Anything shorter keeps the two readings
    // and says why there is no trend. One sample is a reading, not a trend: a
    // single braille column on the right edge reads as a broken plot, so the
    // compact lines carry it until there are two.
    if !wide || samples < 2 || inner.height < 5 || inner.width < 24 {
        f.render_widget(
            Paragraph::new(vec![
                Line::from(vec![
                    Span::styled("↓ ", Style::default().fg(t.ok)),
                    Span::styled(
                        bps(s.traffic.dl_rate_bytes as f64),
                        Style::default().fg(t.ok).add_modifier(Modifier::BOLD),
                    ),
                    Span::raw("   "),
                    Span::styled("↑ ", Style::default().fg(t.accent)),
                    Span::styled(
                        bps(s.traffic.ul_rate_bytes as f64),
                        Style::default().fg(t.accent).add_modifier(Modifier::BOLD),
                    ),
                ]),
                Line::from(Span::styled(
                    format!(
                        "session {} ↓ · {} ↑",
                        bytes(s.traffic.session_dl_bytes),
                        bytes(s.traffic.session_ul_bytes)
                    ),
                    Style::default().fg(t.dim),
                )),
            ]),
            inner,
        );
        return;
    }

    mirrored_body(f, inner, app);
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
                s.signal.band.clone().unwrap_or_else(|| "-".into()),
                s.signal
                    .dl_mhz
                    .map(|v| v.to_string())
                    .unwrap_or_else(|| "-".into()),
                s.signal
                    .pci
                    .map(|p| p.to_string())
                    .unwrap_or_else(|| "-".into()),
                s.signal.tm.clone().unwrap_or_else(|| "-".into()),
                s.clients.len()
            ),
            Style::default().fg(t.fg),
        ),
    ]);
    f.render_widget(Paragraph::new(line), area);
}

fn graphs_panel(f: &mut Frame, area: Rect, app: &App, wide: bool) {
    let t = &app.theme;
    let block = panel(
        format!(
            "SIGNAL GRAPHS · last {}s",
            app.history_rsrp.len() as u64 * app.refresh_ms.max(1) / 1000
        ),
        t,
    );
    let inner = block.inner(area);
    f.render_widget(block, area);
    // The panel owns its rectangle: blank it before drawing, so a glyph left by
    // an earlier frame, a resize or another tab cannot survive as a stray
    // character beside a label. Nothing else paints a background here, so a
    // reset is invisible.
    f.render_widget(Clear, inner);

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

    // Every series gets the rows that are actually left. When there is not even
    // one row per series, print the labels without traces instead of clamping
    // to 1 and letting the last series fall off the panel.
    let avail = (inner.height as usize).saturating_sub(3);
    let rows = (avail / 3).min(4);
    // The traces own their rows: the label, value and domain sit on the line
    // above, so there is no gutter to reserve and the trace runs to the edge.
    let width = (inner.width as usize).max(8);
    // A dot cell holds two columns of samples, so the series is resampled at
    // the cell's own resolution and the renderer's two-per-cell pass leaves no
    // gap. Bars and the ascii fallback take one sample per cell. Mapping the
    // record straight in drew 120 samples as 60 cells in the right quarter.
    let slots = if app.graph.is_bars() || app.graph.is_ascii() {
        width
    } else {
        width * 2
    };
    let secs_per_sample = (app.refresh_ms.max(1) / 1000).max(1);
    let window = plot_window(app);

    let mut lines: Vec<Line> = Vec::new();
    for (label, hist, colour, min, max) in series {
        lines.push(Line::from(vec![
            Span::styled(pad(label, 6), Style::default().fg(t.dim)),
            Span::styled(
                pad(
                    &hist
                        .back()
                        .map(|v| format!("{v:.0}"))
                        .unwrap_or_else(|| "-".to_string()),
                    6,
                ),
                Style::default().fg(colour).add_modifier(Modifier::BOLD),
            ),
            Span::styled(format!("{min:.0}…{max:.0}"), Style::default().fg(t.dim)),
        ]));
        if rows > 0 {
            let mapped = crate::ui::chart::resample_scaled_to_window(
                hist,
                min,
                secs_per_sample,
                window,
                slots,
            );
            lines.extend(crate::ui::chart::spark_lines_scaled(
                &mapped,
                width,
                rows,
                app.graph.is_bars(),
                colour,
                min,
                max,
                app.chart_opts(),
            ));
        }
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

#[cfg(test)]
mod tests {
    use super::{graphs_panel, split, throughput_panel, visible_rows};
    use crate::app::{App, Tab};
    use crate::theme::{GraphStyle, Theme};

    fn fixture_app() -> App {
        let snap: crate::model::Snapshot =
            serde_json::from_str(include_str!("../../fixtures/sample-snapshot.json"))
                .expect("fixture snapshot");
        let mut app = App::new(
            Theme::get("terminal"),
            GraphStyle::Dots,
            crate::app::View::Full,
            100,
            2000,
        );
        app.on_snapshot(snap, 432);
        app.tab = Tab::Network;
        app
    }

    fn app_with_history(n: usize) -> App {
        let mut a = fixture_app();
        for i in 0..n {
            let mut e = crate::history::Entry::blank();
            e.ended_unix = crate::app::unix_now() - i as u64 * 60;
            e.started_unix = e.ended_unix - 20;
            e.download_bps = 120_000_000.0 + i as f64;
            e.upload_bps = 20_000_000.0;
            e.ping_idle_ms = Some(18.0);
            e.ping_loaded_ms = Some(64.0);
            a.history.push(e);
        }
        a.history_total = n;
        a
    }

    fn app_with_speed_result() -> App {
        let mut a = fixture_app();
        a.speed_run += 1;
        let r = crate::speedtest::SpeedResult {
            download_bps: 150_000_000.0,
            at_unix: crate::app::unix_now(),
            ..crate::speedtest::SpeedResult::default()
        };
        let run = crate::worker::SpeedRun {
            result: r,
            elapsed: std::time::Duration::from_secs(12),
            pings: crate::history::Pings::default(),
        };
        a.on_speed_done(a.speed_run, Ok(run));
        a
    }

    fn draw_network(app: &App, w: u16, h: u16) -> String {
        crate::ui::render_to_string(app, w, h)
    }

    #[test]
    fn the_bottom_strip_never_starves_a_panel_and_never_shrinks_growth() {
        let mut last_hist_rows = 0;
        for h in 18..=48u16 {
            let (link, ping, hist) = split(h);
            assert_eq!(
                link + ping + hist,
                h,
                "the three panels fill the body at {h}"
            );
            assert!(link >= 8, "CELL | LINK needs six content lines at {h}");
            assert!(ping >= 7, "the ping panel keeps its minimum at {h}");
            assert!(hist <= 14, "history never takes more than its cap at {h}");
            let rows = visible_rows(hist);
            assert!(rows >= last_hist_rows, "growth must not hide a row: {h}");
            last_hist_rows = rows;
        }
    }

    #[test]
    fn the_table_in_the_plan_matches_the_code() {
        use visible_rows as v;
        assert_eq!((split(18), v(split(18).2)), ((8, 7, 3), 1));
        assert_eq!((split(20), v(split(20).2)), ((8, 7, 5), 3));
        assert_eq!((split(21), v(split(21).2)), ((8, 7, 6), 4));
        assert_eq!((split(24), v(split(24).2)), ((8, 7, 9), 7));
        assert_eq!((split(27), v(split(27).2)), ((8, 7, 12), 10));
        assert_eq!((split(29), v(split(29).2)), ((8, 7, 14), 10));
        assert_eq!((split(33), v(split(33).2)), ((8, 11, 14), 10));
        assert_eq!((split(40), v(split(40).2)), ((15, 11, 14), 10));
    }

    #[test]
    fn the_speed_title_counts_the_rows_that_are_drawn() {
        let mut a = app_with_history(10);
        // 80x24: header 2 + body 21 + footer 1, so four saved rows fit.
        let screen = draw_network(&a, 80, 24);
        assert!(
            screen.contains("4 of 10"),
            "the title must not claim ten drawn rows"
        );
        let screen = draw_network(&a, 100, 30);
        assert!(screen.contains("10 of 10"), "{screen}");
        a.history.truncate(3);
        let screen = draw_network(&a, 100, 30);
        assert!(screen.contains("3 of 10"));
    }

    /// Four rows on screen must not carry the median of ten: the number is
    /// checkable against the lines below it, or it is noise.
    #[test]
    fn the_median_covers_only_the_rows_on_screen() {
        let a = app_with_history(10);
        let full = draw_network(&a, 100, 30);
        let narrow = draw_network(&a, 80, 24);
        assert!(narrow.contains("4 of 10"));
        let med = |screen: &str| -> f64 {
            let at = screen.find("med ").expect("a median") + 4;
            screen[at..]
                .split(' ')
                .next()
                .expect("a number")
                .parse()
                .expect("a float")
        };
        let first_four = a.history[..4].iter().map(|e| e.download_bps).sum::<f64>() / 4.0;
        assert!(
            (med(&narrow) - first_four / 1e6).abs() < 0.2,
            "narrow median {} must be the four drawn rows, not {}",
            med(&narrow),
            med(&full)
        );
    }

    /// A measurement and the key that starts the next one outrank prose. At
    /// 100x14 the panel has three content rows, so only those two survive.
    #[test]
    fn the_ping_panel_keeps_numbers_and_drops_the_note() {
        let mut a = app_with_speed_result();
        a.speed_context = Some(crate::app::SpeedContext {
            net_type: "LTE".into(),
            band: "LTE B3".into(),
            ..Default::default()
        });
        if let Some(r) = a.speed.as_mut() {
            r.note = Some("note that is much longer than the rows allow".into());
        }
        let screen = draw_network(&a, 100, 30);
        assert!(screen.contains("Mbps"), "the rate is a number, never prose");
        assert!(screen.contains("from THIS machine"));
        assert!(screen.contains("through this host"));

        let screen = draw_network(&a, 100, 14);
        assert!(
            screen.contains("Mbps"),
            "a short panel still shows the rate"
        );
        assert!(screen.contains("from THIS machine"));
        assert!(
            !screen.contains("much longer than the rows"),
            "the note is what a three row panel drops"
        );
    }

    #[test]
    fn a_short_terminal_keeps_the_old_two_panel_layout() {
        let a = app_with_history(10);
        let screen = draw_network(&a, 80, 18);
        assert!(screen.contains("PING · SPEED"));
        assert!(
            !screen.contains("SPEED · "),
            "no history panel when the body is under 18 rows"
        );
    }

    // ----------------------------------------------------------- plot window

    /// Render one panel alone into a text grid, so a test can measure the
    /// columns its ink occupies rather than guess from a whole frame.
    fn panel_rows<F>(w: u16, h: u16, draw: F) -> Vec<String>
    where
        F: FnOnce(&mut ratatui::Frame, ratatui::layout::Rect),
    {
        let backend = ratatui::backend::TestBackend::new(w, h);
        let mut terminal = ratatui::Terminal::new(backend).expect("test terminal");
        let area = ratatui::layout::Rect::new(0, 0, w, h);
        terminal.draw(|f| draw(f, area)).expect("draw panel");
        let buf = terminal.backend().buffer().clone();
        (0..h)
            .map(|y| (0..w).map(|x| buf[(x, y)].symbol().to_string()).collect())
            .collect()
    }

    /// The leftmost and rightmost column that carries plot ink between two
    /// rows. Only braille and block glyphs count: the gutter prints scale
    /// numbers on the plot's own rows, and a digit is not a sample.
    fn ink_span(rows: &[String], from: usize, to: usize) -> Option<(usize, usize)> {
        let mut lo: Option<usize> = None;
        let mut hi: Option<usize> = None;
        for line in rows.iter().take(to).skip(from) {
            for (x, ch) in line.chars().enumerate() {
                let plot_glyph = ('\u{2580}'..='\u{259f}').contains(&ch)
                    || ('\u{2801}'..='\u{28ff}').contains(&ch);
                if plot_glyph {
                    lo = Some(lo.map_or(x, |v| v.min(x)));
                    hi = Some(hi.map_or(x, |v| v.max(x)));
                }
            }
        }
        lo.map(|l| (l, hi.expect("ink needs a right edge")))
    }

    /// 120 samples at the 2000ms default: one full 240s window.
    fn full_window_app() -> App {
        let mut a = fixture_app();
        a.history_dl.clear();
        a.history_ul.clear();
        for i in 0..120 {
            a.history_dl.push_back(1_000_000.0 + i as f64);
            a.history_ul.push_back(100_000.0);
        }
        a
    }

    /// The rows the rx series can ink: below the readings line and above the
    /// zero row, which the renderer inks across the whole window by design.
    fn rx_rows(h: u16) -> (usize, usize) {
        let plot_h = h as usize - 4;
        let rx_h = plot_h - plot_h / 2;
        (2, 2 + rx_h - 1)
    }

    #[test]
    fn the_throughput_plot_spans_the_window_at_every_size() {
        let app = full_window_app();
        for (w, h) in [(80u16, 24u16), (120, 34), (220, 50)] {
            let rows = panel_rows(w, h, |f, a| throughput_panel(f, a, &app));
            let (from, to) = rx_rows(h);
            let (lo, hi) = ink_span(&rows, from, to).expect("plot ink");
            assert!(
                lo <= 14,
                "the window opens at the plot's left edge at {w}x{h}: {lo}"
            );
            assert!(
                hi >= w as usize - 3,
                "the newest sample lands on the plot's right edge at {w}x{h}: {hi}"
            );
        }
    }

    #[test]
    fn the_throughput_axis_ticks_sit_on_the_window_edges() {
        let app = full_window_app();
        for (w, h) in [(120u16, 34u16), (220, 50)] {
            let rows = panel_rows(w, h, |f, a| throughput_panel(f, a, &app));
            let axis = &rows[h as usize - 2];
            let left = axis.find("-240s").expect("window start tick");
            let mid = axis.find("-120s").expect("window midpoint tick");
            let now = axis.rfind("now").expect("now tick");
            assert!(
                left <= 14,
                "the start tick sits on the plot's left edge: {axis}"
            );
            let centre = left + (w as usize - 2 - left) / 2;
            assert!(
                (mid as i32 - centre as i32).abs() <= 2,
                "the midpoint tick sits mid-plot at {w}x{h}: {axis}"
            );
            assert!(
                now + 3 >= w as usize - 2,
                "the now tick sits on the right edge at {w}x{h}: {axis}"
            );
        }
    }

    #[test]
    fn a_short_throughput_history_keeps_the_window() {
        let mut app = fixture_app();
        app.history_dl.clear();
        app.history_ul.clear();
        for _ in 0..30 {
            app.history_dl.push_back(1_000_000.0);
            app.history_ul.push_back(100_000.0);
        }
        let (w, h) = (220u16, 50u16);
        let rows = panel_rows(w, h, |f, a| throughput_panel(f, a, &app));
        let axis = &rows[h as usize - 2];
        assert!(
            !axis.contains("-240s"),
            "no tick may name time the record never covered: {axis}"
        );
        let start = axis.find("-60s").expect("the record start on the ruler");
        assert!(axis.contains("now"), "the ruler still ends at now: {axis}");
        let (from, to) = rx_rows(h);
        let (lo, hi) = ink_span(&rows, from, to).expect("plot ink");
        assert!(hi >= w as usize - 3, "the newest sample sits at now: {hi}");
        // 60s of a 240s window: the ink starts three quarters in, and the tick
        // naming that instant sits on the same column as the first sample.
        assert!(
            lo >= w as usize * 7 / 10,
            "a quarter-full record must not stretch across the window: ink starts at {lo}"
        );
        // Nor may it be squeezed to the right edge: 60s of 240s begins at three
        // quarters of the window, not at the last few columns.
        assert!(
            lo <= w as usize * 4 / 5,
            "the record keeps its true age: ink starts at {lo}"
        );
        assert!(
            (lo as i32 - start as i32).abs() <= 3,
            "the record's first column and its tick must agree: ink {lo}, tick {start}"
        );
    }

    #[test]
    fn the_signal_graphs_span_the_window() {
        for (w, h) in [(120u16, 34u16), (220, 50)] {
            let mut app = fixture_app();
            app.history_rsrp.clear();
            for i in 0..120 {
                app.history_rsrp
                    .push_back(if i % 2 == 0 { -140.0 } else { -44.0 });
            }
            let rows = panel_rows(w, h, |f, a| graphs_panel(f, a, &app, true));
            let inner_h = h as usize - 2;
            let traces = ((inner_h - 3) / 3).min(4);
            // Row 1 is the RSRP label; the traces follow; the last of them is
            // the baseline, which is inked even with no data.
            let (lo, _) = ink_span(&rows, 2, traces + 1).expect("trace ink");
            assert!(
                lo <= 2,
                "the trace fills the window from its left edge at {w}x{h}: {lo}"
            );
        }
    }

    #[test]
    fn a_short_signal_history_stays_on_the_right() {
        let (w, h) = (220u16, 50u16);
        let mut app = fixture_app();
        app.history_rsrp.clear();
        for i in 0..30 {
            app.history_rsrp
                .push_back(if i % 2 == 0 { -140.0 } else { -44.0 });
        }
        let rows = panel_rows(w, h, |f, a| graphs_panel(f, a, &app, true));
        let inner_h = h as usize - 2;
        let traces = ((inner_h - 3) / 3).min(4);
        let (lo, hi) = ink_span(&rows, 2, traces + 1).expect("trace ink");
        assert!(
            lo >= w as usize / 2,
            "60s of a 240s window starts on the right half: ink starts at {lo}"
        );
        assert!(
            lo <= w as usize * 4 / 5,
            "the record keeps its true age instead of hugging the right edge: ink starts at {lo}"
        );
        assert!(hi >= w as usize - 3, "the newest sample sits at now: {hi}");
    }
}
