//! Small drawing helpers. No widget library beyond ratatui primitives, so the
//! braille sparkline can render the same "dots" look netwatch uses.

use crate::theme::GraphStyle;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use std::collections::VecDeque;

pub fn bytes(v: u64) -> String {
    const K: f64 = 1024.0;
    let f = v as f64;
    if f >= K * K * K * K {
        format!("{:.2} TB", f / (K * K * K * K))
    } else if f >= K * K * K {
        format!("{:.2} GB", f / (K * K * K))
    } else if f >= K * K {
        format!("{:.1} MB", f / (K * K))
    } else if f >= K {
        format!("{:.0} KB", f / K)
    } else {
        format!("{v} B")
    }
}

pub fn bps(v: f64) -> String {
    let bits = v * 8.0;
    if bits >= 1e9 {
        format!("{:.2} Gbps", bits / 1e9)
    } else if bits >= 1e6 {
        format!("{:.2} Mbps", bits / 1e6)
    } else if bits >= 1e3 {
        format!("{:.1} Kbps", bits / 1e3)
    } else {
        format!("{bits:.0} bps")
    }
}

pub fn dur(secs: u64) -> String {
    if secs >= 30 * 86_400 {
        format!("{}d", secs / 86_400)
    } else if secs >= 86_400 {
        format!("{}d {}h", secs / 86_400, (secs % 86_400) / 3600)
    } else if secs >= 3600 {
        format!("{}h {}m", secs / 3600, (secs % 3600) / 60)
    } else if secs >= 60 {
        format!("{}m", secs / 60)
    } else {
        format!("{secs}s")
    }
}

pub fn opt_f(v: Option<f32>, unit: &str) -> String {
    match v {
        Some(x) => format!("{x:.0} {unit}"),
        None => "-".to_string(),
    }
}

pub fn opt_s(v: &Option<String>) -> String {
    v.clone().unwrap_or_else(|| "-".into())
}

pub fn kv<'a>(label: &'a str, value: String, value_style: Style, dim: Color) -> Line<'a> {
    Line::from(vec![
        Span::styled(format!("{label:<10}"), Style::default().fg(dim)),
        Span::styled(value, value_style),
    ])
}

/// A horizontal fill bar, e.g. a local data budget.
pub fn bar<'a>(fraction: f64, width: usize, color: Color, dim: Color) -> Line<'a> {
    let filled = ((fraction.clamp(0.0, 1.0)) * width as f64).round() as usize;
    let mut s = String::new();
    for i in 0..width {
        s.push(if i < filled { '█' } else { '░' });
    }
    Line::from(vec![
        Span::styled(s, Style::default().fg(color)),
        Span::raw(" "),
        Span::styled(
            format!("{:.0}%", fraction * 100.0),
            Style::default().fg(dim),
        ),
    ])
}

/// Sparkline scaled to a FIXED range (used for signal metrics, where the
/// physical domain is known). Auto-scaling made a single sample paint one column
/// at the right edge - invisible - and made two series incomparable.
pub fn spark_scaled<'a>(
    values: &VecDeque<f64>,
    width: usize,
    height: usize,
    style: GraphStyle,
    color: Color,
    min: f64,
    max: f64,
) -> Vec<Line<'a>> {
    if width == 0 || height == 0 {
        return Vec::new();
    }
    match style {
        GraphStyle::Bars => vec![bars_row_scaled(values, width, color, min, max)],
        _ => braille_scaled(values, width, height, color, min, max),
    }
}

/// Segmented threshold ruler: the bar itself carries the quality zones, and the
/// fill shows where the reading sits. Replaces the circular gauge for signal
/// metrics (circles stay for download/upload).
/// Domain + threshold zones for a `ruler`. Grouped so the call stays readable.
pub struct RulerSpec {
    pub min: f64,
    pub max: f64,
    pub poor_until: f64,
    pub fair_until: f64,
    pub width: usize,
}

pub fn ruler<'a>(value: f64, spec: &RulerSpec, ok: Color, warn: Color, danger: Color) -> Line<'a> {
    let (min, max, width) = (spec.min, spec.max, spec.width);
    let span = (max - min).max(1e-9);
    let frac = ((value - min) / span).clamp(0.0, 1.0);
    let filled = (frac * width as f64).round() as usize;

    // ONE colour for the whole row: the quality of this reading. A rainbow of
    // zones made a "fair" row look red, orange and green at once.
    let colour = if value < spec.poor_until {
        danger
    } else if value < spec.fair_until {
        warn
    } else {
        ok
    };

    // Threshold positions, so the zones stay readable without colouring the row.
    let tick_at = |bound: f64| -> Option<usize> {
        let f = (bound - min) / span;
        if f > 0.0 && f < 1.0 {
            Some((f * width as f64).round() as usize)
        } else {
            None
        }
    };
    let ticks: Vec<usize> = [spec.poor_until, spec.fair_until]
        .iter()
        .filter_map(|b| tick_at(*b))
        .collect();

    let mut spans: Vec<Span> = Vec::new();
    let mut run = String::new();
    let mut run_filled = true;
    for i in 0..width {
        let now_filled = i < filled;
        // Ticks only mark the EMPTY part, so the fill stays a solid block.
        let is_tick = ticks.contains(&i) && !now_filled;
        if now_filled != run_filled {
            spans.push(if run_filled {
                Span::styled(
                    std::mem::take(&mut run),
                    Style::default().fg(colour).add_modifier(Modifier::BOLD),
                )
            } else {
                Span::styled(std::mem::take(&mut run), Style::default().fg(colour))
            });
            run_filled = now_filled;
        }
        if is_tick {
            run.push('┊');
        } else {
            run.push(if now_filled { '█' } else { '░' });
        }
    }
    spans.push(if run_filled {
        Span::styled(
            run,
            Style::default().fg(colour).add_modifier(Modifier::BOLD),
        )
    } else {
        Span::styled(run, Style::default().fg(colour))
    });
    Line::from(spans)
}

fn bars_row_scaled<'a>(
    values: &VecDeque<f64>,
    width: usize,
    color: Color,
    min: f64,
    max: f64,
) -> Line<'a> {
    let data: Vec<f64> = values.iter().rev().take(width).rev().copied().collect();
    let span = (max - min).max(1e-9);
    let mut s = String::new();
    for _ in 0..width.saturating_sub(data.len()) {
        s.push('·');
    }
    for v in &data {
        let idx = (((v - min) / span).clamp(0.0, 1.0) * 8.0).round() as usize;
        s.push(LEVELS[idx]);
    }
    Line::from(Span::styled(s, Style::default().fg(color)))
}

fn braille_scaled<'a>(
    values: &VecDeque<f64>,
    width: usize,
    height: usize,
    color: Color,
    min: f64,
    max: f64,
) -> Vec<Line<'a>> {
    let gw = width * 2;
    let gh = height * 4;
    let span = (max - min).max(1e-9);
    let data: Vec<f64> = values.iter().rev().take(gw).rev().copied().collect();
    let mut bits = vec![vec![false; gw]; gh];
    let offset = gw.saturating_sub(data.len());
    for (i, v) in data.iter().enumerate() {
        let x = offset + i;
        let norm = ((v - min) / span).clamp(0.0, 1.0);
        let filled = ((norm * gh as f64).round() as usize).clamp(1, gh);
        for r in 0..filled {
            bits[gh - 1 - r][x] = true;
        }
    }
    let mut lines = Vec::with_capacity(height);
    for row in 0..height {
        let mut s = String::with_capacity(width);
        for col in 0..width {
            let mut code = 0x2800u32;
            for dy in 0..4 {
                for dx in 0..2 {
                    if bits[row * 4 + dy][col * 2 + dx] {
                        code |= match (dx, dy) {
                            (0, 0) => 0x01,
                            (0, 1) => 0x02,
                            (0, 2) => 0x04,
                            (0, 3) => 0x40,
                            (1, 0) => 0x08,
                            (1, 1) => 0x10,
                            (1, 2) => 0x20,
                            (1, 3) => 0x80,
                            _ => 0,
                        };
                    }
                }
            }
            s.push(char::from_u32(code).unwrap_or(' '));
        }
        lines.push(Line::from(Span::styled(s, Style::default().fg(color))));
    }
    lines
}

/// Sparkline in one of the three styles the plan promises.
pub fn spark<'a>(
    values: &VecDeque<f64>,
    width: usize,
    height: usize,
    style: GraphStyle,
    color: Color,
) -> Vec<Line<'a>> {
    if width == 0 || height == 0 {
        return Vec::new();
    }
    match style {
        GraphStyle::Bars => vec![bars_row(values, width, color)],
        _ => braille(values, width, height, color),
    }
}

const LEVELS: [char; 9] = [' ', '▁', '▂', '▃', '▄', '▅', '▆', '▇', '█'];

fn bars_row<'a>(values: &VecDeque<f64>, width: usize, color: Color) -> Line<'a> {
    let data: Vec<f64> = values.iter().rev().take(width).rev().copied().collect();
    let max = data.iter().cloned().fold(0.0_f64, f64::max).max(1e-9);
    let mut s = String::new();
    for _ in 0..width.saturating_sub(data.len()) {
        s.push(' ');
    }
    for v in &data {
        let idx = ((v / max) * 8.0).round().clamp(0.0, 8.0) as usize;
        s.push(LEVELS[idx]);
    }
    Line::from(Span::styled(s, Style::default().fg(color)))
}

/// Braille canvas: 2 dots horizontally and 4 vertically per cell, filled from the
/// bottom, newest value at the right edge.
fn braille<'a>(values: &VecDeque<f64>, width: usize, height: usize, color: Color) -> Vec<Line<'a>> {
    let gw = width * 2;
    let gh = height * 4;
    let data: Vec<f64> = values.iter().rev().take(gw).rev().copied().collect();
    let max = data.iter().cloned().fold(0.0_f64, f64::max).max(1e-9);
    let mut bits = vec![vec![false; gw]; gh];
    let offset = gw.saturating_sub(data.len());
    for (i, v) in data.iter().enumerate() {
        let x = offset + i;
        let filled = (((v / max) * gh as f64).round() as usize).min(gh);
        for r in 0..filled {
            bits[gh - 1 - r][x] = true;
        }
    }
    let mut lines = Vec::with_capacity(height);
    for row in 0..height {
        let mut s = String::with_capacity(width);
        for col in 0..width {
            let mut code = 0x2800u32;
            for dy in 0..4 {
                for dx in 0..2 {
                    if bits[row * 4 + dy][col * 2 + dx] {
                        code |= match (dx, dy) {
                            (0, 0) => 0x01,
                            (0, 1) => 0x02,
                            (0, 2) => 0x04,
                            (0, 3) => 0x40,
                            (1, 0) => 0x08,
                            (1, 1) => 0x10,
                            (1, 2) => 0x20,
                            (1, 3) => 0x80,
                            _ => 0,
                        };
                    }
                }
            }
            s.push(char::from_u32(code).unwrap_or(' '));
        }
        lines.push(Line::from(Span::styled(s, Style::default().fg(color))));
    }
    lines
}

pub fn title_style(color: Color) -> Style {
    Style::default().fg(color).add_modifier(Modifier::BOLD)
}

/// A radial gauge drawn on a braille dot grid.
///
/// The sweep runs 135° → 405° (bottom-left, up the left side, over the top, down
/// the right) so the gap sits at the bottom like a physical meter. The filled part
/// is the accent colour, the rest is dimmed, and a needle marks the exact value.
/// Dot coordinates are normalised per-axis (2 dots per cell horizontally, 4
/// vertically), which renders round in a normal terminal cell ratio.
pub fn radial_gauge<'a>(
    value: f64,
    min: f64,
    max: f64,
    width: usize,
    height: usize,
    fill: Color,
    dim: Color,
) -> Vec<Line<'a>> {
    if width < 4 || height < 2 {
        return Vec::new();
    }
    let frac = if max > min {
        ((value - min) / (max - min)).clamp(0.0, 1.0)
    } else {
        0.0
    };
    let gw = width * 2;
    let gh = height * 4;
    let cx = (gw as f64 - 1.0) / 2.0;
    let cy = (gh as f64 - 1.0) / 2.0;
    let rx = (gw as f64 - 1.0) / 2.0;
    let ry = (gh as f64 - 1.0) / 2.0;

    // 1 = empty arc (dim), 2 = filled arc (accent), 3 = needle
    let mut cells = vec![vec![0u8; gw]; gh];
    let needle_angle = 135.0 + 270.0 * frac;

    for (y, row) in cells.iter_mut().enumerate() {
        for (x, cell) in row.iter_mut().enumerate() {
            let dx = (x as f64 - cx) / rx;
            let dy = (y as f64 - cy) / ry;
            let r = (dx * dx + dy * dy).sqrt();
            if !(0.86..=1.02).contains(&r) {
                continue;
            }
            let mut angle = dy.atan2(dx).to_degrees();
            if angle < 135.0 {
                angle += 360.0;
            }
            if (135.0..=405.0).contains(&angle) {
                *cell = if angle <= needle_angle { 2 } else { 1 };
            }
        }
    }

    // Needle: from the centre out to 82% of the radius.
    let steps = gw.max(gh) * 2;
    for i in 0..=steps {
        let t = i as f64 / steps as f64;
        let r = t * 0.82;
        let a = needle_angle.to_radians();
        let x = (cx + r * a.cos() * rx) as isize;
        let y = (cy + r * a.sin() * ry) as isize;
        if x >= 0 && y >= 0 && (x as usize) < gw && (y as usize) < gh {
            cells[y as usize][x as usize] = 3;
        }
    }

    // kind -> (colour, bold): the arc's filled part and the needle share the
    // accent colour, so runs only break on kind == 1 versus kind >= 2.
    let style_for = |kind: u8| -> (Color, bool) {
        match kind {
            0 => (dim, false),
            1 => (dim, false),
            3 => (fill, true),
            _ => (fill, false),
        }
    };

    let mut lines = Vec::with_capacity(height);
    for row in 0..height {
        let mut spans: Vec<Span> = Vec::new();
        let mut run = String::new();
        let mut run_key: Option<(Color, bool)> = None;
        for col in 0..width {
            let (code, kind) = braille_cell(&cells, row, col);
            let key = style_for(kind);
            if Some(key) != run_key {
                if let Some((colour, bold)) = run_key {
                    let mut style = Style::default().fg(colour);
                    if bold {
                        style = style.add_modifier(Modifier::BOLD);
                    }
                    spans.push(Span::styled(std::mem::take(&mut run), style));
                }
                run_key = Some(key);
            }
            run.push(code);
        }
        if let Some((colour, bold)) = run_key {
            let mut style = Style::default().fg(colour);
            if bold {
                style = style.add_modifier(Modifier::BOLD);
            }
            spans.push(Span::styled(run, style));
        }
        lines.push(Line::from(spans));
    }
    lines
}

fn braille_cell(cells: &[Vec<u8>], row: usize, col: usize) -> (char, u8) {
    let mut code = 0x2800u32;
    let mut strongest = 0u8;
    for dy in 0..4 {
        for dx in 0..2 {
            let y = row * 4 + dy;
            let x = col * 2 + dx;
            if y >= cells.len() || x >= cells[y].len() {
                continue;
            }
            let v = cells[y][x];
            if v != 0 {
                strongest = strongest.max(v);
                code |= match (dx, dy) {
                    (0, 0) => 0x01,
                    (0, 1) => 0x02,
                    (0, 2) => 0x04,
                    (0, 3) => 0x40,
                    (1, 0) => 0x08,
                    (1, 1) => 0x10,
                    (1, 2) => 0x20,
                    (1, 3) => 0x80,
                    _ => 0,
                };
            }
        }
    }
    (char::from_u32(code).unwrap_or(' '), strongest)
}

/// Display width, so Arabic host names (double-width in most fonts' cell count)
/// do not break column alignment.
pub fn width_of(s: &str) -> usize {
    unicode_width::UnicodeWidthStr::width(s)
}

/// Pad to an exact display width, cutting from the end when the text is wider.
pub fn pad(s: &str, width: usize) -> String {
    let mut out = String::new();
    let mut used = 0;
    for c in s.chars() {
        let w = unicode_width::UnicodeWidthChar::width(c).unwrap_or(0);
        if used + w > width {
            break;
        }
        out.push(c);
        used += w;
    }
    while used < width {
        out.push(' ');
        used += 1;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn byte_and_rate_formatting() {
        assert_eq!(bytes(0), "0 B");
        assert_eq!(bytes(1024), "1 KB");
        assert_eq!(bytes(1024 * 1024), "1.0 MB");
        assert!(bytes(1024 * 1024 * 1024 * 1024).ends_with("TB"));
        assert_eq!(bps(125_000.0), "1.00 Mbps");
    }

    #[test]
    fn braille_renders_requested_size() {
        let mut v = VecDeque::new();
        for i in 0..50 {
            v.push_back(i as f64);
        }
        let lines = spark(&v, 20, 3, GraphStyle::Braille, Color::Reset);
        assert_eq!(lines.len(), 3);
        let text: String = lines
            .iter()
            .map(|l| {
                l.spans
                    .iter()
                    .map(|s| s.content.clone())
                    .collect::<String>()
            })
            .collect();
        assert_eq!(text.chars().count(), 60, "20 cells x 3 rows");
        let painted = text.chars().filter(|c| *c != '\u{2800}').count();
        assert!(painted > 20, "expected filled cells, got {painted}");
        // newest sample sits at the right edge, so the last column must be painted
        let last_col_painted = (0..3).any(|row| {
            text.chars()
                .nth(row * 20 + 19)
                .map(|c| c != '\u{2800}')
                .unwrap_or(false)
        });
        assert!(last_col_painted, "right edge must show the newest value");
    }

    #[test]
    fn bars_are_one_row() {
        let v: VecDeque<f64> = (0..10).map(|i| i as f64).collect();
        let lines = spark(&v, 10, 3, GraphStyle::Bars, Color::Reset);
        assert_eq!(lines.len(), 1);
    }

    #[test]
    fn ruler_marks_the_value_and_paints_zones() {
        let spec = RulerSpec {
            min: -140.0,
            max: -44.0,
            poor_until: -100.0,
            fair_until: -80.0,
            width: 40,
        };
        let line = ruler(
            -96.0,
            &spec,
            Color::Indexed(2),
            Color::Indexed(3),
            Color::Indexed(1),
        );
        let text: String = line.spans.iter().map(|s| s.content.clone()).collect();
        assert_eq!(
            text.chars().count(),
            40,
            "the ruler is exactly `width` cells"
        );
        let filled = text.chars().filter(|c| *c == '█').count();
        assert_eq!(filled, 18, "(-96 + 140) / 96 = 46% of 40 cells");
        assert!(
            text.contains('┊'),
            "threshold marks must stay visible: {text}"
        );
        let colours: std::collections::BTreeSet<String> = line
            .spans
            .iter()
            .map(|s| format!("{:?}", s.style.fg))
            .collect();
        assert_eq!(
            colours.len(),
            1,
            "one colour per row, not a rainbow: {colours:?}"
        );

        // out-of-range values clamp instead of overflowing the bar
        let wide = RulerSpec { width: 20, ..spec };
        let low = ruler(
            -200.0,
            &wide,
            Color::Indexed(2),
            Color::Indexed(3),
            Color::Indexed(1),
        );
        let low_text: String = low.spans.iter().map(|s| s.content.clone()).collect();
        assert_eq!(low_text.chars().filter(|c| *c == '█').count(), 0);
        assert_eq!(
            low.spans[0].style.fg,
            Some(Color::Indexed(1)),
            "out-of-range low must read as danger"
        );
        let high = ruler(
            0.0,
            &wide,
            Color::Indexed(2),
            Color::Indexed(3),
            Color::Indexed(1),
        );
        let high_text: String = high.spans.iter().map(|s| s.content.clone()).collect();
        assert_eq!(high_text.chars().filter(|c| *c == '█').count(), 20);
        assert_eq!(
            high.spans[0].style.fg,
            Some(Color::Indexed(2)),
            "top of range reads good"
        );
    }

    #[test]
    fn fixed_range_spark_shows_a_single_sample() {
        let mut v = VecDeque::new();
        v.push_back(-96.0);
        let lines = spark_scaled(&v, 20, 2, GraphStyle::Braille, Color::Reset, -140.0, -44.0);
        let last: String = lines
            .last()
            .unwrap()
            .spans
            .iter()
            .map(|s| s.content.clone())
            .collect();
        assert_eq!(last.chars().count(), 20, "full width is rendered");
        assert_ne!(
            last.chars().last(),
            Some('\u{2800}'),
            "the newest sample must paint the right edge: {last:?}"
        );

        // a fixed domain also means the same value paints the same height
        let mut many = VecDeque::new();
        for _ in 0..40 {
            many.push_back(-96.0);
        }
        let steady = spark_scaled(
            &many,
            20,
            2,
            GraphStyle::Braille,
            Color::Reset,
            -140.0,
            -44.0,
        );
        let painted: usize = steady
            .iter()
            .flat_map(|l| l.spans.iter())
            .flat_map(|s| s.content.chars())
            .filter(|c| *c != '\u{2800}')
            .count();
        // -96 sits at 46% of the -140..-44 domain: the bottom line only
        assert_eq!(
            painted, 20,
            "the lower half is filled, the upper half is not"
        );

        let mut strong = VecDeque::new();
        for _ in 0..40 {
            strong.push_back(-44.0);
        }
        let full = spark_scaled(
            &strong,
            20,
            2,
            GraphStyle::Braille,
            Color::Reset,
            -140.0,
            -44.0,
        );
        let painted: usize = full
            .iter()
            .flat_map(|l| l.spans.iter())
            .flat_map(|s| s.content.chars())
            .filter(|c| *c != '\u{2800}')
            .count();
        assert_eq!(painted, 40, "a top-of-range value fills both lines");
    }

    #[test]
    fn radial_gauge_geometry() {
        let lines = radial_gauge(-96.0, -140.0, -44.0, 9, 4, Color::Reset, Color::Reset);
        assert_eq!(lines.len(), 4, "height in cells");
        for line in &lines {
            let text: String = line.spans.iter().map(|s| s.content.clone()).collect();
            assert_eq!(text.chars().count(), 9, "width in cells: {text:?}");
        }
        let painted = lines
            .iter()
            .flat_map(|l| l.spans.iter())
            .flat_map(|s| s.content.chars())
            .filter(|c| *c != '\u{2800}')
            .count();
        assert!(painted > 12, "the arc must be drawn, painted={painted}");
    }

    #[test]
    fn radial_gauge_saturates_and_tolerates_tiny_areas() {
        let low = radial_gauge(0.0, 0.0, 100.0, 8, 3, Color::Reset, Color::Reset);
        let high = radial_gauge(999.0, 0.0, 100.0, 8, 3, Color::Reset, Color::Reset);
        assert_eq!(low.len(), high.len());
        assert!(radial_gauge(1.0, 0.0, 1.0, 2, 1, Color::Reset, Color::Reset).is_empty());
    }

    #[test]
    fn padding_counts_display_columns() {
        // Arabic host names must not shift the columns after them.
        let name = "الشاشة";
        assert_eq!(width_of(&pad(name, 20)), 20);
        assert_eq!(width_of(&pad("tv", 20)), 20);
        assert_eq!(pad("abcdef", 3), "abc");
        assert_eq!(width_of(&pad("غرفة المعيشة", 12)), 12);
    }
}
