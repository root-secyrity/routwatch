//! The key help: one table, two lengths, grouped under a heading per section,
//! and a window that scrolls instead of drawing past its own panel.
//!
//! The table is the only place a key is described. `short` must fit a 34 column
//! panel and is what a narrow terminal sees; `full` is used only when the room
//! exists. The chosen text is wrapped, never cut: what does not fit the
//! description column continues on the next line, indented under it, so a long
//! description is read to its end instead of ending in an ellipsis. The rows are
//! grouped: a `Heading` names a section and the keys under it are that section's,
//! so a row never has to say which section it belongs to.
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
//! The grouped list is taller than a normal terminal, so the overlay scrolls.
//! `help_off` is the index of the first visible SOURCE row into `rows()` (a
//! heading is one row, and a key row that draws two lines is still one row) and
//! `help_cap` is how many source rows were drawn. Both are read by the scroll
//! handler in `src/main.rs`, and the renderer is the only thing that measures the
//! capacity, so it records it back through the `Cell` it is handed.
//!
//! The panel is measured in DRAWN lines, the window moves in whole source rows: a
//! row whose wrapped description does not fit is left for the next window rather
//! than drawn in half.
//!
//! The wrapping is held by `a_help_row_wraps_instead_of_being_cut` and
//! `a_wrapped_row_continues_under_the_description_column`, and the band by
//! `a_heading_is_a_filled_bar_and_a_key_row_is_not`, all in this module;
//! `the_help_never_cuts_a_description_it_can_wrap` and
//! `the_panel_is_tall_enough_for_the_wrapped_list_at_120x56` in `ui::tests` hold the
//! same facts through a rendered frame, and
//! `inverse_reads_on_the_title_slot_in_every_palette` in `theme::tests` holds the
//! band readable on every palette this build ships.

use std::cell::Cell;

use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Clear, Paragraph};
use ratatui::Frame;

use crate::theme::Theme;

use super::tabs::{truncate, wrap_words};

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
/// The list is grouped under a heading per section: 45 source rows in all. How
/// many terminal lines that needs depends on the width, because a description is
/// wrapped rather than cut, so the widest panel draws some rows on two lines. A
/// terminal shorter than the drawn list scrolls.
pub fn rows() -> Vec<HelpRow> {
    use HelpRow::{Heading, Key};
    vec![
        Heading("GLOBAL KEYS"),
        Key(KeyRow {
            keys: "0..9 / Tab",
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
    // The room alone decides which of the two lengths is drawn; the text is
    // wrapped, so `full` never has to be cut to fit.
    let use_full = room >= 66;

    // The panel is measured in the lines the rows actually draw: a heading is one
    // line, and a key row is as many lines as its wrapped description needs.
    let drawn: Vec<usize> = all
        .iter()
        .map(|row| match row {
            HelpRow::Heading(_) => 1,
            HelpRow::Key(k) => row_lines(k, key_w, room, use_full, theme).len(),
        })
        .collect();

    let want = drawn.iter().sum::<usize>() as u16 + 3; // border, content, footer line
    let h = want.min(area.height).max(area.height.min(5));
    let rect = Rect {
        x: area.x + (area.width.saturating_sub(w)) / 2,
        y: area.y + (area.height.saturating_sub(h)) / 2,
        width: w,
        height: h,
    };

    let total = all.len();
    let mut off = (off as usize).min(total.saturating_sub(1));
    let body_cap = h.saturating_sub(3) as usize;

    // The window moves in whole source rows, and one line is reserved for the
    // position line while rows are left over: a row whose description does not fit
    // is left for the next window rather than drawn in half.
    let mut shown = 0usize;
    let mut used = 0usize;
    for (i, lines) in drawn.iter().enumerate().skip(off) {
        let reserve = usize::from(i + 1 < total);
        if used + lines + reserve > body_cap {
            break;
        }
        used += lines;
        shown += 1;
    }
    // A window that lands on the last row is grown backwards while it still fits,
    // so a reader who scrolls to the end sees a screenful and not the last row:
    // the position line is still needed, so one line stays reserved.
    while off + shown == total && off > 0 && used + 1 + drawn[off - 1] <= body_cap {
        off -= 1;
        used += drawn[off];
        shown += 1;
    }
    shown = shown.max(1);
    let end = off + shown;
    // The position line belongs to any window that is not the whole list, at the
    // top, in the middle, or resting on the last row.
    let windowed = off > 0 || end < total;
    cap.set(shown as u16);

    let visible = &all[off..end];

    let mut lines: Vec<Line> = Vec::with_capacity(used + 2);
    for row in visible {
        match row {
            HelpRow::Heading(text) => lines.push(heading_line(text, inner, theme)),
            HelpRow::Key(k) => lines.extend(row_lines(k, key_w, room, use_full, theme)),
        }
    }
    // A heading whose key rows all fell outside the window is not drawn: a
    // heading with nothing under it says nothing.
    if matches!(visible.last(), Some(HelpRow::Heading(_))) {
        lines.pop();
    }

    if windowed {
        // The last source row whose lines were drawn: the trailing heading is
        // counted out when it was popped, and the message speaks 1-based rows.
        let last = if lines.len() < used { end - 1 } else { end };
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

/// A section heading is a filled bar, not coloured text: two palette slots can
/// resolve to the same colour in a terminal theme, so the band is what tells a
/// reader where one section ends and the next begins. It is padded to `inner` so
/// the fill runs the width of the panel.
fn heading_line(text: &str, inner: u16, theme: &Theme) -> Line<'static> {
    let label = format!(" {} ", truncate(text, inner.saturating_sub(2) as usize));
    let fill = " ".repeat((inner as usize).saturating_sub(label.chars().count()));
    Line::from(Span::styled(
        format!("{label}{fill}"),
        Style::default()
            .fg(theme.inverse)
            .bg(theme.title)
            .add_modifier(Modifier::BOLD),
    ))
}

/// The lines one key row draws. The description is wrapped, never cut, so a long
/// one continues on the next line indented under the description column; the keys
/// cell is still cut to `key_w`, because a key group that ran into its description
/// would read as one word.
fn row_lines(
    row: &KeyRow,
    key_w: u16,
    room: usize,
    use_full: bool,
    theme: &Theme,
) -> Vec<Line<'static>> {
    let text = if use_full { row.full } else { row.short };
    let mut chunks = wrap_words(text, room);
    if chunks.is_empty() {
        chunks.push(String::new());
    }
    let keys = truncate(row.keys, key_w as usize);
    let pad = " ".repeat(key_w as usize + 2);
    chunks
        .into_iter()
        .enumerate()
        .map(|(i, chunk)| {
            let lead = if i == 0 {
                Span::styled(
                    format!(" {keys:<width$} ", width = key_w as usize),
                    Style::default()
                        .fg(theme.accent)
                        .add_modifier(Modifier::BOLD),
                )
            } else {
                Span::styled(pad.clone(), Style::default().fg(theme.fg))
            };
            Line::from(vec![
                lead,
                Span::styled(chunk, Style::default().fg(theme.fg)),
            ])
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The room a 120 column panel gives a description, and the rooms an 80
    /// column one and the narrowest panel give.
    const WIDE: usize = 66;
    const NARROW: usize = 18;

    #[test]
    fn a_help_row_wraps_instead_of_being_cut() {
        // A description is read to its end: at the widest panel every row's text
        // comes back whole when the chunks are put back together, and no chunk on
        // any width is wider than the room it was wrapped to.
        for row in key_rows() {
            for text in [row.short, row.full] {
                let chunks = wrap_words(text, WIDE);
                assert!(!chunks.is_empty(), "{:?} wrapped to nothing", row.keys);
                assert_eq!(chunks.join(" "), text, "{:?} lost text at {WIDE}", row.keys);
            }
            for room in [NARROW, 38, WIDE] {
                let chunks = wrap_words(row.full, room);
                assert!(!chunks.is_empty(), "{:?} wrapped to nothing", row.keys);
                for chunk in &chunks {
                    assert!(
                        chunk.chars().count() <= room,
                        "{:?} drew {} columns in a {room} column room",
                        row.keys,
                        chunk.chars().count()
                    );
                }
            }
        }
    }

    #[test]
    fn a_wrapped_row_continues_under_the_description_column() {
        // The row the owner's screenshot cut: its description runs past one line,
        // so the rest is drawn under the description column and the whole text is
        // still there, with no ellipsis.
        let theme = Theme::get("terminal");
        let key_w = 12u16;
        let row = key_rows()
            .into_iter()
            .find(|r| r.keys == "d" && r.full.len() > WIDE)
            .expect("the delete row is longer than one description line");
        let lines = row_lines(&row, key_w, WIDE, true, &theme);
        assert!(lines.len() > 1, "the row must wrap, not cut");
        let mut drawn = Vec::new();
        for (i, line) in lines.iter().enumerate() {
            let lead = line.spans[0].content.to_string();
            if i == 0 {
                assert_eq!(lead.chars().count(), key_w as usize + 2, "keys cell");
            } else {
                assert_eq!(lead, " ".repeat(key_w as usize + 2), "continuation indent");
            }
            drawn.push(line.spans[1].content.to_string());
        }
        assert_eq!(drawn.join(" "), row.full);
        assert!(!drawn.join(" ").contains('…'), "a description is never cut");
    }

    #[test]
    fn a_heading_is_a_filled_bar_and_a_key_row_is_not() {
        let theme = Theme::get("terminal");
        let inner = 80u16;
        let head = heading_line("BANDS (Tab 2)", inner, &theme);
        assert_eq!(head.spans.len(), 1, "a heading is one span");
        let band = &head.spans[0];
        assert_eq!(
            band.style.bg,
            Some(theme.title),
            "the band is the title slot"
        );
        assert_eq!(
            band.style.fg,
            Some(theme.inverse),
            "the label is drawn on the band"
        );
        assert_eq!(
            band.content.chars().count(),
            inner as usize,
            "the band runs the width of the panel"
        );
        for row in key_rows() {
            for line in row_lines(&row, 12, WIDE, true, &theme) {
                for span in &line.spans {
                    assert_eq!(span.style.bg, None, "a key row is not a band");
                }
            }
        }
    }
}
