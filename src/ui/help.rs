//! The key help: one table, two lengths, grouped under a heading per section,
//! and a window that scrolls instead of drawing past its own panel.
//!
//! The table is the only place a key is described. `short` must fit a 34 column
//! panel and is what a narrow terminal sees; `full` is used only when the room
//! exists. The rows are grouped: a `Heading` names a section and the keys under
//! it are that section's, so a row never has to say which section it belongs to.
//!
//! Every key `handle_key` answers to should have a row here, but no one test
//! holds the whole list together: `the_help_lists_the_speed_key` and
//! `the_renew_row_is_in_the_help_and_the_device_hint` (both in `ui::tests`) each
//! check a single row, and the retired-key test in `src/main.rs` only forbids the
//! rows of a key the handler no longer answers to. The grouping itself is held by
//! `every_help_heading_has_a_row_under_it`, `the_help_names_every_section_it_has`
//! and `the_help_groups_the_keys_under_a_heading` in `ui::tests`. A new key with
//! no row here fails nothing today.
//!
//! The grouped list is taller than a normal terminal, so the overlay scrolls:
//! `help_off` is the index of the first visible line into `rows()` (a heading
//! counts as one line) and `help_cap` is how many content lines were actually
//! shown. Both are read by the scroll handler in `src/main.rs`, and the renderer
//! is the only thing that measures the capacity, so it records it back through
//! the `Cell` it is handed.

use std::cell::Cell;

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

/// A line of the help table: either a section heading or a key row. A heading
/// owns every key row under it until the next heading.
pub enum HelpRow {
    Heading(&'static str),
    Key(KeyRow),
}

/// The whole key list, in the order it is shown, with the section headings that
/// group it. Global keys come first: a user who is lost needs `t`, `p`, `P` and
/// `?` before anything else.
///
/// Ten headings, 35 key rows, 45 lines: 49 rows of terminal show all of it and
/// anything shorter scrolls.
pub fn rows() -> Vec<HelpRow> {
    use HelpRow::{Heading, Key};
    vec![
        Heading("GLOBAL KEYS"),
        Key(KeyRow {
            keys: "1..9 / Tab",
            short: "switch section",
            full: "switch section from any view",
        }),
        Key(KeyRow {
            keys: "j k / ↑ ↓",
            short: "move the selection",
            full: "move the selection",
        }),
        Key(KeyRow {
            keys: "t",
            short: "theme: next",
            full: "theme: next of the nine; t in settings does the same",
        }),
        Key(KeyRow {
            keys: "p",
            short: "ping a host",
            full: "ping a host from this machine, not through the router",
        }),
        Key(KeyRow {
            keys: "P",
            short: "pause / resume polling",
            full: "pause / resume polling (capital P, from any section)",
        }),
        Key(KeyRow {
            keys: "S / ,",
            short: "settings overlay",
            full: "settings: theme, view, refresh, budget; w saves",
        }),
        Key(KeyRow {
            keys: "[ / ]",
            short: "refresh period ±0.5 s",
            full: "refresh period in 0.5 s steps, from any view",
        }),
        Key(KeyRow {
            keys: "v / V",
            short: "view: full/gauges/lite/dense",
            full: "cycle the view: full -> gauges -> lite -> dense",
        }),
        Key(KeyRow {
            keys: "l / L",
            short: "lite on / off",
            full: "toggle lite and return to the view you came from",
        }),
        Key(KeyRow {
            keys: "i",
            short: "event feed",
            full: "event feed: j/k reads back, space pauses, Esc closes",
        }),
        Key(KeyRow {
            keys: "r",
            short: "refresh now",
            full: "refresh now; Shift+r reads the extras on SMS/Device",
        }),
        Key(KeyRow {
            keys: "e",
            short: "report · SMS/Device: extras",
            full: "write a Markdown report; on SMS/Device e reads the extras instead",
        }),
        Key(KeyRow {
            keys: "?",
            short: "this help",
            full: "this help",
        }),
        Key(KeyRow {
            keys: "q / Ctrl+C",
            short: "quit",
            full: "quit (logs out)",
        }),
        Heading("SETTINGS (overlay)"),
        Key(KeyRow {
            keys: "- / +",
            short: "budget ±10 GB",
            full: "traffic budget in 10 GB steps, inside settings",
        }),
        Key(KeyRow {
            keys: "w",
            short: "save the settings",
            full: "save the settings to the config file",
        }),
        Key(KeyRow {
            keys: "i",
            short: "report identity",
            full: "include the device identity in the report the next time one is written",
        }),
        Heading("DASHBOARD (Tab 1)"),
        Key(KeyRow {
            keys: "Enter / s / f",
            short: "open · sort · filter",
            full: "move the clients table: Enter opens the row, s sorts, f filters; j k move",
        }),
        Heading("BANDS (Tab 2)"),
        Key(KeyRow {
            keys: "space",
            short: "tick a band",
            full: "tick a band; in the event feed it pauses",
        }),
        Key(KeyRow {
            keys: "a / y",
            short: "apply, then y",
            full: "apply the ticked merge; y confirms, n or Esc cancels",
        }),
        Key(KeyRow {
            keys: "u",
            short: "undo the apply",
            full: "put the mask the last apply replaced back in the tick list",
        }),
        Key(KeyRow {
            keys: "n / s / x",
            short: "preset next/save/del",
            full: "next merge preset / save the ticked merge / delete it",
        }),
        Key(KeyRow {
            keys: "m / o",
            short: "mode / NR option",
            full: "network mode / NR option",
        }),
        Heading("TRAFFIC (Tab 3)"),
        Key(KeyRow {
            keys: "b",
            short: "reset budget",
            full: "reset the budget baseline (asks first; the day table stays)",
        }),
        Heading("NETWORK (Tab 4)"),
        Key(KeyRow {
            keys: "T",
            short: "speed test",
            full: "download/upload test through this host; it spends data",
        }),
        Heading("CLIENTS (Tab 5)"),
        Key(KeyRow {
            keys: "s / c",
            short: "sort / copy MAC",
            full: "sort by name/ip/band/time / copy the MAC",
        }),
        Heading("SMS (Tab 6)"),
        Key(KeyRow {
            keys: "Enter",
            short: "open a message",
            full: "open the message under the cursor in the wrapped reader; Esc closes it. Opening never marks it read",
        }),
        Key(KeyRow {
            keys: "space",
            short: "mark / unmark",
            full: "mark or unmark the row under the cursor; a marks every row and clears them again, and the mark count sits in the panel title",
        }),
        Key(KeyRow {
            keys: "n / p",
            short: "next / prev page",
            full: "next / previous inbox page (20 rows a page, refused when there is no more)",
        }),
        Key(KeyRow {
            keys: "d",
            short: "delete, then y",
            full: "delete the marked rows (or the one under the cursor); asks y first, needs a writable build, and reports every index that failed",
        }),
        Key(KeyRow {
            keys: "m",
            short: "mark read, then y",
            full: "mark the open message read on the device; only by this key, never by opening it",
        }),
        Heading("DEVICE (Tab 7)"),
        Key(KeyRow {
            keys: "x / b",
            short: "dump / restart",
            full: "dump the endpoints to JSON / restart (type REBOOT)",
        }),
        Key(KeyRow {
            keys: "W",
            short: "renew the WAN address",
            full: "renew: the data connection drops for a few seconds and the ISP may keep the lease",
        }),
        Heading("DNS (Tab 8)"),
        Key(KeyRow {
            keys: "Enter",
            short: "apply the preset",
            full: "apply the chosen preset (then y)",
        }),
        Key(KeyRow {
            keys: "d / z",
            short: "type servers / DHCP",
            full: "type servers / back to DHCP; the same pair answers on traffic",
        }),
    ]
}

/// The key rows alone, without the headings. What a reader that has no frame
/// wants: the three tests that ask whether a key has a row.
pub fn key_rows() -> Vec<KeyRow> {
    rows()
        .into_iter()
        .filter_map(|row| match row {
            HelpRow::Key(k) => Some(k),
            HelpRow::Heading(_) => None,
        })
        .collect()
}

/// The line under the table. It says what this build can actually do, instead
/// of claiming read-only on a build that writes.
pub fn footer(allow_write: bool) -> &'static str {
    if allow_write {
        " writes enabled: a then y sends to the router"
    } else {
        " read-only build: start with --allow-write to write"
    }
}

/// Draw the overlay centred over `area`. Never draws a line wider than the
/// panel. `off` is the first visible line of `rows()`, and the capacity the
/// panel could show is recorded through `cap` for the scroll handler.
pub fn draw(
    f: &mut Frame,
    area: Rect,
    theme: &Theme,
    allow_write: bool,
    off: u16,
    cap: &Cell<u16>,
) {
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

    let want = all.len() as u16 + 3; // border, content, footer line
    let h = want.min(area.height).max(area.height.min(5));
    let rect = Rect {
        x: area.x + (area.width.saturating_sub(w)) / 2,
        y: area.y + (area.height.saturating_sub(h)) / 2,
        width: w,
        height: h,
    };

    // The window is measured in content lines, and a heading is one of them.
    let body_cap = h.saturating_sub(3) as usize;
    let windowed = all.len() > body_cap;
    let shown = body_cap.saturating_sub(usize::from(windowed)).max(1);
    cap.set(shown as u16);

    let total = all.len();
    let off = (off as usize).min(total.saturating_sub(shown));
    let end = (off + shown).min(total);
    let visible = &all[off..end];

    let mut lines: Vec<Line> = Vec::with_capacity(shown + 1);
    for row in visible {
        match row {
            HelpRow::Heading(text) => lines.push(heading_line(text, inner, theme)),
            HelpRow::Key(k) => lines.push(row_line(k, key_w, room, use_full, theme)),
        }
    }
    // A heading whose key rows all fell outside the window is not drawn: a
    // heading with nothing under it says nothing.
    if matches!(visible.last(), Some(HelpRow::Heading(_))) {
        lines.pop();
    }

    if windowed {
        let last = if lines.len() < visible.len() {
            end - 1
        } else {
            end
        };
        let msg = format!(
            " rows {}-{last} of {total} · j k / PgUp PgDn scroll",
            off + 1
        );
        lines.push(Line::from(Span::styled(
            truncate(&msg, inner as usize),
            Style::default().fg(theme.dim),
        )));
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

fn heading_line(text: &str, inner: u16, theme: &Theme) -> Line<'static> {
    Line::from(Span::styled(
        format!(" {}", truncate(text, inner.saturating_sub(1) as usize)),
        Style::default()
            .fg(theme.title)
            .add_modifier(Modifier::BOLD),
    ))
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
