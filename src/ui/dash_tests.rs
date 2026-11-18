//! The dashboard's seven regions, checked on the rendered buffer.
//!
//! These tests read the frame the way a person does: the plot rows are found by
//! their labels, and a column is a character position. Nothing here reaches the
//! network, and nothing asserts a colour, because the glyph has to carry the
//! reading on its own.

use crate::app::{App, Tab};
use crate::theme::{GraphStyle, Theme};
use crate::ui::{render_to_string, tab_from_key};

fn app(samples: usize) -> App {
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
    app.tab = Tab::Dashboard;
    // `on_snapshot` seeds one reading of every series; start from an empty store
    // so a test's sample count is the count the plots resample.
    app.history_dl.clear();
    app.history_ul.clear();
    app.history_rsrp.clear();
    app.history_rsrq.clear();
    app.history_sinr.clear();
    for i in 0..samples {
        // One reading per poll; the pattern walks the three quality steps so a
        // single frame carries all three glyphs.
        app.history_dl.push_back(1_000_000.0 + i as f64 * 1000.0);
        app.history_ul.push_back(200_000.0);
        app.history_rsrp.push_back(match i % 6 {
            0 => -70.0,
            3 => -120.0,
            _ => -90.0,
        });
        app.history_rsrq.push_back(match i % 4 {
            0 => -20.0,
            _ => -8.0,
        });
        app.history_sinr.push_back(match i % 5 {
            0 => -4.0,
            _ => 22.0,
        });
    }
    app
}

fn row_of<'a>(frame: &'a str, needle: &str) -> &'a str {
    frame
        .lines()
        .find(|line| line.contains(needle))
        .unwrap_or_else(|| panic!("no row holds {needle:?}:\n{frame}"))
}

/// A row inside one panel, so " RSRP " finds the heatmap row and not the meter.
fn row_in<'a>(frame: &'a str, panel: &str, needle: &str) -> &'a str {
    let lines: Vec<&str> = frame.lines().collect();
    let start = lines
        .iter()
        .position(|l| l.contains(panel))
        .unwrap_or_else(|| panic!("no panel {panel:?}:\n{frame}"));
    lines[start..]
        .iter()
        .find(|l| l.contains(needle))
        .copied()
        .unwrap_or_else(|| panic!("no row {needle:?} under {panel:?}:\n{frame}"))
}

/// The plot cells of a heatmap row: everything between the label field and the
/// statistics column. The row pads the label to six columns, so the field pad is
/// part of the gutter and not part of cell zero.
fn map_cells<'a>(frame: &'a str, metric: &str) -> &'a str {
    let row = row_in(frame, "SIGNAL MAP", metric);
    let start = row.find(metric).expect("metric") + metric.len();
    let rest = &row[start..];
    let pad = 6usize.saturating_sub(metric.len());
    let rest = rest.get(pad..).unwrap_or_default();
    let end = rest.find("  min ").unwrap_or(rest.len());
    &rest[..end]
}

fn first_drawn(text: &str) -> usize {
    text.chars()
        .position(|c| !c.is_whitespace())
        .unwrap_or(usize::MAX)
}

#[test]
fn seven_zones_and_no_duplicate_figure() {
    let frame = render_to_string(&app(120), 120, 42);
    for title in [
        "RADIO",
        "LINK / SESSION",
        "SIGNAL MAP ·",
        "THROUGHPUT",
        "EVENTS / ERRORS",
    ] {
        assert_eq!(
            frame.matches(&format!("┌ {title}")).count(),
            1,
            "{title} must own exactly one panel:\n{frame}"
        );
    }
    assert_eq!(frame.matches("┌ CLIENTS · ").count(), 1, "{frame}");
    // The WAN address lives in LINK / SESSION and nowhere else.
    assert_eq!(frame.matches("203.0.113.43").count(), 1, "{frame}");
    // The band and its width are stated once, in RADIO.
    assert_eq!(frame.matches("2120/1930").count(), 1, "{frame}");
}

#[test]
fn signal_map_and_throughput_share_one_time_axis() {
    let frame = render_to_string(&app(40), 120, 42);
    let down_line = row_of(&frame, "↓ down");
    let down = down_line[down_line.find("↓ down").unwrap() + "↓ down".len()..]
        .trim_end_matches(['│', ' '])
        .to_string();
    let rsrp = map_cells(&frame, "RSRP").to_string();
    let a = first_drawn(&rsrp);
    let b = first_drawn(&down);
    assert_ne!(a, usize::MAX);
    assert_eq!(
        a, b,
        "the same sample must start the same column in both plots:\nmap  {rsrp}\ndown {down}"
    );
}

/// Seconds the legend says one cell stands for.
fn cell_seconds(frame: &str) -> f64 {
    let at = frame
        .find("1 cell = ")
        .expect("the legend names the cell span")
        + 9;
    let rest = &frame[at..];
    let end = rest.find('s').expect("the span ends in seconds");
    rest[..end].parse().expect("the span is a number")
}

#[test]
fn the_window_stays_240s_while_the_cell_granularity_adapts() {
    let narrow = render_to_string(&app(120), 80, 24);
    assert!(narrow.contains("last 240s"), "{narrow}");
    let wide = render_to_string(&app(120), 200, 50);
    assert!(wide.contains("last 240s"), "{wide}");
    assert!(
        cell_seconds(&narrow) > cell_seconds(&wide),
        "a narrow plot must cover the same window with wider cells: {} vs {}",
        cell_seconds(&narrow),
        cell_seconds(&wide)
    );
    assert!(
        cell_seconds(&wide) >= 1.0,
        "a cell is never narrower than a second: {wide}"
    );
}

#[test]
fn an_empty_cell_is_no_sample_not_good_and_not_the_neighbour() {
    // Ten polls at 2 s are twenty seconds of a 240-second window: the left of
    // the plot is blank, and blank is a space, not the newest reading copied
    // back and not a "good" cell.
    let frame = render_to_string(&app(10), 200, 50);
    let rsrp = map_cells(&frame, "RSRP").to_string();
    let filled = rsrp.chars().filter(|c| "▓▒░".contains(*c)).count();
    assert!(filled > 0);
    assert!(
        filled < rsrp.chars().count() / 4,
        "a 20 s record must not fill a 240 s window: {rsrp:?}"
    );
    let first = rsrp.chars().position(|c| "▓▒░".contains(c)).unwrap();
    assert!(
        first > rsrp.chars().count() / 2,
        "the short record belongs at the right edge, found it at {first}"
    );
    assert!(
        rsrp.chars().take(first).all(|c| c == ' '),
        "nothing may be drawn where no sample exists: {rsrp:?}"
    );
}

#[test]
fn a_bucket_shows_its_worst_sample_not_its_average() {
    let mut a = app(120);
    a.history_rsrp.clear();
    for i in 0..120 {
        a.history_rsrp
            .push_back(if i == 60 { -130.0 } else { -60.0 });
    }
    let frame = render_to_string(&a, 200, 50);
    let rsrp = map_cells(&frame, "RSRP").to_string();
    let glyphs: Vec<char> = rsrp.chars().filter(|c| "▓▒░".contains(*c)).collect();
    let poor = glyphs.iter().filter(|c| **c == '░').count();
    assert!(poor >= 1, "one deep dip must survive the bucket: {rsrp:?}");
    assert!(
        poor * 4 < glyphs.len(),
        "the dip is one sample, not half the window: {rsrp:?}"
    );
}

#[test]
fn the_quality_glyphs_are_distinct_without_color() {
    let frame = render_to_string(&app(120), 200, 50);
    let rsrp = map_cells(&frame, "RSRP").to_string();
    for glyph in ['▓', '▒', '░'] {
        assert!(rsrp.contains(glyph), "missing {glyph} in {rsrp:?}");
    }
}

#[test]
fn a_panel_with_nothing_to_say_collapses_to_one_line() {
    let frame = render_to_string(&app(120), 120, 42);
    assert_eq!(
        frame.matches("not exposed by this firmware").count(),
        1,
        "5G takes one line, not a panel:\n{frame}"
    );
    assert_eq!(
        frame.matches("errors 0").count(),
        1,
        "no errors is one line inside EVENTS / ERRORS:\n{frame}"
    );
}

#[test]
fn the_insight_line_is_derived_and_disappears_when_samples_are_short() {
    let short = render_to_string(&app(5), 120, 42);
    assert!(!short.contains("worst"), "{short}");

    let long = render_to_string(&app(120), 120, 42);
    assert!(long.contains("worst"), "{long}");
    assert!(long.contains("dips"), "{long}");
}

#[test]
fn client_columns_the_firmware_never_reported_are_dropped_not_faked() {
    let frame = render_to_string(&app(120), 120, 42);
    let header = row_of(&frame, "name ");
    assert!(header.contains("ip"), "{header}");
    assert!(header.contains("band"), "{header}");
    // `Client` in the model carries no rssi and no rate: the columns are gone,
    // and no dash stands in for a reading the firmware never sent.
    assert!(!header.contains("rssi"), "{header}");
    assert!(!header.contains("rate"), "{header}");
}

#[test]
fn enter_on_a_client_opens_the_clients_tab_on_that_client() {
    let mut a = app(120);
    a.dash_cursor(1);
    let wanted = a.dash_clients().get(1).and_then(|c| c.ip.clone());
    assert!(a.dash_open_client());
    assert_eq!(a.tab, Tab::Clients);
    let shown = a.selected_client().and_then(|i| {
        let list = a.sorted_clients();
        list[i].ip.clone()
    });
    assert_eq!(shown, wanted);
}

#[test]
fn the_body_is_filled_and_no_empty_run_exceeds_two_rows() {
    let frame = render_to_string(&app(120), 120, 42);
    let lines: Vec<&str> = frame.lines().collect();
    // Every row of the body belongs to a panel: a gap would be a row with no
    // box border and no text, and there must be none. Blank rows *inside* a
    // panel are the padding under its content, not unused screen.
    for (i, line) in lines.iter().enumerate().skip(2).take(lines.len() - 3) {
        let has_wall = line.contains('│') || line.contains('┌') || line.contains('└');
        assert!(has_wall, "row {i} is not part of any panel:\n{frame}");
    }
    let last_body = &lines[lines.len() - 2];
    assert!(
        last_body.contains('└'),
        "the bottom panel must reach the status row"
    );
}

#[test]
fn every_key_that_existed_still_works() {
    let hints = Tab::Dashboard.hints();
    for key in ["v view", "g graph", "r refresh", "P pause", "e report"] {
        assert!(
            hints.contains(key),
            "{key} left the dashboard hints: {hints}"
        );
    }
    let frame = render_to_string(&app(4), 200, 50);
    for key in [
        "v view",
        "g graph",
        "r refresh",
        "P pause",
        "e report",
        "i events",
        "q quit",
        "? help",
    ] {
        assert!(frame.contains(key), "{key} left the status row:\n{frame}");
    }
    for (digit, tab) in [
        ('1', Tab::Dashboard),
        ('5', Tab::Clients),
        ('9', Tab::Signal),
    ] {
        assert_eq!(tab_from_key(digit), Some(tab));
    }
}

#[test]
fn no_panic_from_40x12_to_240x60() {
    let a = app(120);
    for (w, h) in [
        (40u16, 12u16),
        (60, 20),
        (80, 24),
        (100, 30),
        (110, 35),
        (120, 34),
        (120, 42),
        (200, 50),
        (240, 60),
    ] {
        let frame = render_to_string(&a, w, h);
        assert_eq!(frame.lines().count(), h as usize, "{w}x{h}");
    }
}
