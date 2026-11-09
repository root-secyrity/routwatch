//! The key help: one table, two lengths, and a layout that never draws past its
//! own panel.
//!
//! The table is the only place a key is described. `short` must fit a 34 column
//! panel and is what a narrow terminal sees; `full` is used only when the room
//! exists. Every key `handle_key` answers to has a row here, and the test
//! `help_covers_every_key_the_handler_answers_to` holds the two together.

use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Clear, Paragraph};
use ratatui::Frame;

use crate::theme::Theme;

use super::tabs::truncate;

/// Column the keys are padded to. Longer key groups are cut, never allowed to
/// run into their description.
const KEY_COL: u16 = 12;

/// One line of the help. `keys` is what the user presses.
pub struct KeyRow {
    pub keys: &'static str,
    pub short: &'static str,
    pub full: &'static str,
}

/// The whole key list, in the order it is shown. The last two rows (`?` and
/// `q`) are pinned by the renderer: on a short terminal they always survive.
pub fn rows() -> Vec<KeyRow> {
    vec![
        KeyRow {
            keys: "1..9 / Tab",
            short: "switch section",
            full: "switch section from any view",
        },
        KeyRow {
            keys: "j k / ↑ ↓",
            short: "move the selection",
            full: "move the selection",
        },
        KeyRow {
            keys: "space",
            short: "bands: tick a band",
            full: "bands: tick a band; in the event feed it pauses",
        },
        KeyRow {
            keys: "a / y",
            short: "bands: apply, then y",
            full: "bands: apply the ticked merge; y confirms, n or Esc cancels",
        },
        KeyRow {
            keys: "u",
            short: "bands: undo the apply",
            full: "bands: put the mask the last apply replaced back in the tick list",
        },
        KeyRow {
            keys: "n / s / x",
            short: "bands: preset next/save/del",
            full: "bands: next merge preset / save the ticked merge / delete it",
        },
        KeyRow {
            keys: "m / o",
            short: "bands: mode / NR option",
            full: "bands: network mode / NR option",
        },
        KeyRow {
            keys: "d / z",
            short: "DNS: type servers / DHCP",
            full: "DNS (8) and traffic: type servers / back to DHCP",
        },
        KeyRow {
            keys: "Enter",
            short: "DNS: apply the preset",
            full: "DNS: apply the chosen preset (then y)",
        },
        KeyRow {
            keys: "s / c",
            short: "clients: sort / copy MAC",
            full: "clients: sort by name/ip/band/time / copy the MAC",
        },
        KeyRow {
            keys: "p",
            short: "network: ping a host",
            full: "network (4): ping a host from this machine, not through the router",
        },
        KeyRow {
            keys: "e",
            short: "report · SMS/Device: extras",
            full: "write a Markdown report; on SMS/Device e reads the extras instead",
        },
        KeyRow {
            keys: "x / b",
            short: "device: dump / restart",
            full: "device: dump the endpoints to JSON / restart (type REBOOT)",
        },
        KeyRow {
            keys: "t",
            short: "theme: next",
            full: "theme: next of the nine; t in settings does the same",
        },
        KeyRow {
            keys: "S / ,",
            short: "settings overlay",
            full: "settings: theme, view, graph, refresh, budget; w saves",
        },
        KeyRow {
            keys: "[ / ]",
            short: "refresh period ±0.5 s",
            full: "refresh period in 0.5 s steps, from any view",
        },
        KeyRow {
            keys: "- / +",
            short: "budget ±10 GB (settings)",
            full: "traffic budget in 10 GB steps, inside settings",
        },
        KeyRow {
            keys: "v / V",
            short: "view: full/gauges/lite/dense",
            full: "cycle the view: full -> gauges -> lite -> dense",
        },
        KeyRow {
            keys: "l / L",
            short: "lite on / off",
            full: "toggle lite and return to the view you came from",
        },
        KeyRow {
            keys: "g",
            short: "graph: dots/bars/braille",
            full: "cycle the graph: dots -> bars -> braille",
        },
        KeyRow {
            keys: "i",
            short: "event feed",
            full: "event feed: j/k reads back, space pauses, Esc closes",
        },
        KeyRow {
            keys: "r",
            short: "refresh now",
            full: "refresh now; Shift+r reads the extras on SMS/Device",
        },
        KeyRow {
            keys: "P",
            short: "pause / resume polling",
            full: "pause / resume polling (p does this too, except on Network)",
        },
        KeyRow {
            keys: "?",
            short: "this help",
            full: "this help",
        },
        KeyRow {
            keys: "q / Ctrl+C",
            short: "quit",
            full: "quit (logs out)",
        },
    ]
}

/// The line under the table. It says what this build can actually do, instead
/// of claiming read-only on a build that writes.
pub fn footer(allow_write: bool) -> &'static str {
    if allow_write {
        " writes enabled: a then y sends to the router"
    } else {
        " read-only build: no router writes are enabled"
    }
}

/// Draw the overlay centred over `area`. Never draws a line wider than the
/// panel, and keeps the `?` and `q` rows when the terminal is too short.
pub fn draw(f: &mut Frame, area: Rect, theme: &Theme, allow_write: bool) {
    let all = rows();
    let w = area
        .width
        .saturating_sub(6)
        .clamp(34, 82)
        .min(area.width.max(1));
    let inner = w.saturating_sub(2);
    let key_w = KEY_COL.min(inner.saturating_sub(10));
    let room = inner.saturating_sub(key_w + 2).max(4) as usize;
    // `full` is used only when it genuinely fits, so no description is ever cut
    // in the middle of a word without the ellipsis saying so.
    let use_full = room >= 66;

    let want = all.len() as u16 + 3; // border, rows, footer line
    let h = want.min(area.height).max(area.height.min(5));
    let rect = Rect {
        x: area.x + (area.width.saturating_sub(w)) / 2,
        y: area.y + (area.height.saturating_sub(h)) / 2,
        width: w,
        height: h,
    };

    let body_cap = h.saturating_sub(3) as usize;
    let tail = 2usize.min(body_cap);
    let needs_marker = all.len() > body_cap;
    let head = body_cap.saturating_sub(tail + usize::from(needs_marker));

    let mut lines: Vec<Line> = Vec::new();
    for row in all.iter().take(head) {
        lines.push(row_line(row, key_w, room, use_full, theme));
    }
    if needs_marker {
        let hidden = all.len().saturating_sub(head + tail);
        let msg = format!(" … {hidden} more rows: resize the terminal");
        lines.push(Line::from(Span::styled(
            truncate(&msg, inner as usize),
            Style::default().fg(theme.dim),
        )));
    }
    for row in all.iter().skip(all.len() - tail) {
        lines.push(row_line(row, key_w, room, use_full, theme));
    }
    lines.push(Line::from(Span::styled(
        truncate(footer(allow_write), inner as usize),
        Style::default().fg(theme.dim),
    )));

    f.render_widget(Clear, rect);
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

fn row_line(row: &KeyRow, key_w: u16, room: usize, use_full: bool, theme: &Theme) -> Line<'static> {
    let keys = truncate(row.keys, key_w as usize);
    let text = if use_full { row.full } else { row.short };
    Line::from(vec![
        Span::styled(
            format!(" {keys:<width$} ", width = key_w as usize),
            Style::default()
                .fg(theme.accent)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(truncate(text, room), Style::default().fg(theme.fg)),
    ])
}
