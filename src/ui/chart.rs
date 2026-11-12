//! Shared chart rendering: one braille renderer, one ramp, one time window.
//!
//! Behaviour ported from the family's `graph` module (netwatch 0.40.0, MIT):
//! a cell is coloured by how high it sits in the plot, the rx/tx pair grows
//! away from a single zero row, and a column is the same moment in every
//! series because the samples are resampled onto one window first.
//!
//! No `App`, no `Theme`, no network: a `Buffer`, a `Rect`, plain samples and
//! colours in, pixels out.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};
use std::collections::VecDeque;

/// A five-stop colour ramp, sampled 0.0..=1.0 along the axis it colours.
pub struct Ramp {
    stops: Vec<Color>,
}

impl Ramp {
    pub fn new(stops: Vec<Color>) -> Self {
        Self { stops }
    }

    /// One colour, no gradient: what a series looks like with fade off.
    pub fn flat(c: Color) -> Self {
        Self { stops: vec![c] }
    }

    pub fn at(&self, f: f32) -> Color {
        match self.stops.len() {
            0 => Color::Reset,
            1 => self.stops[0],
            n => {
                let t = f.clamp(0.0, 1.0) * (n - 1) as f32;
                let i = t.floor() as usize;
                if i >= n - 1 {
                    return self.stops[n - 1];
                }
                lerp(self.stops[i], self.stops[i + 1], t - i as f32)
            }
        }
    }
}

/// Cross-cutting preferences, built once per frame from `App::chart_opts`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ChartOpts {
    /// Colour each cell by its height in the plot.
    pub fade: bool,
    /// Theme background, the "fade to" anchor.
    pub bg: Color,
    /// True under the `terminal` theme, where no colour may be synthesised.
    pub terminal_palette: bool,
    /// Single-width ASCII glyphs instead of braille or blocks.
    pub ascii: bool,
    /// Draw a floor dot under a zero sample, so quiet reads as quiet.
    pub baseline: bool,
}

impl Default for ChartOpts {
    fn default() -> Self {
        Self {
            fade: true,
            bg: Color::Reset,
            terminal_palette: false,
            ascii: false,
            baseline: true,
        }
    }
}

/// Bottom of the magnitude gradient: the fraction of the series colour a sample
/// sitting on the baseline receives.
const MIN_FADE_ALPHA: f32 = 0.30;
/// Least contrast the dimmest stop may have with the background.
const MIN_RAMP_CONTRAST: f64 = 4.0;
/// Least contrast the series colour may have, so the stops above it can climb.
const MIN_SERIES_CONTRAST: f64 = 6.0;
/// What a ramp is measured against when the terminal owns the background.
const ASSUMED_DARK_BG: (u8, u8, u8) = (0x1a, 0x1b, 0x26);

/// The magnitude gradient: dim at the baseline, the series colour through the
/// body, lightened at the peak. A cell's colour comes from how high it sits, so
/// the shape stays legible where the fill is continuous.
pub fn magnitude_ramp(base: Color, bg: Color) -> Ramp {
    let (r, g, b) = to_rgb(base, (255, 255, 255));
    let behind = to_rgb(bg, ASSUMED_DARK_BG);
    let base = with_contrast(Color::Rgb(r, g, b), behind, MIN_SERIES_CONTRAST);
    let floor = (0..=70)
        .map(|step| MIN_FADE_ALPHA + step as f32 / 100.0)
        .find(|&alpha| contrast(fade_color(base, bg, alpha), behind) >= MIN_RAMP_CONTRAST)
        .unwrap_or(1.0);
    // The peak moves away from the background, not toward white: on a light
    // panel (paper) "brighter" fades into the page, and the measured contrast
    // of the top stops fell to 2.5:1 before this.
    let peak = away_from(behind);
    Ramp::new(vec![
        fade_color(base, bg, floor),
        fade_color(base, bg, floor + (1.0 - floor) * 0.46),
        base,
        lerp(base, peak, 0.42),
        lerp(base, peak, 0.76),
    ])
}

/// The palette's own two steps, for the `terminal` theme: no colour is
/// synthesised, so a 16-colour terminal gets the series token and its bright
/// variant instead of an RGB blend.
pub fn palette_ramp(base: Color) -> Ramp {
    let bright = bright_token(base);
    Ramp::new(vec![base, base, base, bright, bright])
}

fn bright_token(c: Color) -> Color {
    match c {
        Color::Black => Color::DarkGray,
        Color::Red => Color::LightRed,
        Color::Green => Color::LightGreen,
        Color::Yellow => Color::LightYellow,
        Color::Blue => Color::LightBlue,
        Color::Cyan => Color::LightCyan,
        Color::Gray => Color::White,
        Color::DarkGray => Color::Gray,
        Color::Indexed(n) if n < 8 => Color::Indexed(n + 8),
        other => other,
    }
}

/// `base` interpolated toward `bg` at `alpha`. RGB only.
pub fn fade_color(base: Color, bg: Color, alpha: f32) -> Color {
    let alpha = alpha.clamp(0.0, 1.0);
    let (br, bgc, bb) = to_rgb(base, (255, 255, 255));
    let (gr, gg, gb) = to_rgb(bg, (0, 0, 0));
    Color::Rgb(
        lerp_u8(gr, br, alpha),
        lerp_u8(gg, bgc, alpha),
        lerp_u8(gb, bb, alpha),
    )
}

fn to_rgb(c: Color, fallback: (u8, u8, u8)) -> (u8, u8, u8) {
    // xterm's palette values. Needed because the `dark` theme uses named
    // colours: without the mapping every ramp would fade toward one fallback
    // and every chart would read as "fade off".
    match c {
        Color::Rgb(r, g, b) => (r, g, b),
        Color::Reset => fallback,
        Color::Black => (0, 0, 0),
        Color::Red => (170, 0, 0),
        Color::Green => (0, 170, 0),
        Color::Yellow => (170, 85, 0),
        Color::Blue => (0, 0, 170),
        Color::Magenta => (170, 0, 170),
        Color::Cyan => (0, 170, 170),
        Color::Gray => (170, 170, 170),
        Color::DarkGray => (85, 85, 85),
        Color::LightRed => (255, 85, 85),
        Color::LightGreen => (85, 255, 85),
        Color::LightYellow => (255, 255, 85),
        Color::LightBlue => (85, 85, 255),
        Color::LightMagenta => (255, 85, 255),
        Color::LightCyan => (85, 255, 255),
        Color::White => (255, 255, 255),
        _ => fallback,
    }
}

/// Linear blend, RGB only: a named colour comes back unchanged rather than
/// guessing what the user's palette says it is.
pub fn lerp(a: Color, b: Color, f: f32) -> Color {
    match (rgb(a), rgb(b)) {
        (Some((ar, ag, ab)), Some((br, bgc, bb))) => {
            let f = f.clamp(0.0, 1.0);
            Color::Rgb(lerp_u8(ar, br, f), lerp_u8(ag, bgc, f), lerp_u8(ab, bb, f))
        }
        _ => a,
    }
}

fn lerp_u8(a: u8, b: u8, t: f32) -> u8 {
    (a as f32 + (b as f32 - a as f32) * t)
        .round()
        .clamp(0.0, 255.0) as u8
}

fn rgb(c: Color) -> Option<(u8, u8, u8)> {
    match c {
        Color::Rgb(r, g, b) => Some((r, g, b)),
        _ => None,
    }
}

/// WCAG relative luminance.
fn luminance((r, g, b): (u8, u8, u8)) -> f64 {
    let linear = |v: u8| {
        let v = v as f64 / 255.0;
        if v <= 0.04045 {
            v / 12.92
        } else {
            ((v + 0.055) / 1.055).powf(2.4)
        }
    };
    0.2126 * linear(r) + 0.7152 * linear(g) + 0.0722 * linear(b)
}

/// WCAG contrast ratio.
fn contrast(c: Color, behind: (u8, u8, u8)) -> f64 {
    let a = luminance(to_rgb(c, behind));
    let b = luminance(behind);
    (a.max(b) + 0.05) / (a.min(b) + 0.05)
}

/// White on a dark background, black on a light one: the direction that adds
/// contrast for a colour sitting on `behind`.
fn away_from(behind: (u8, u8, u8)) -> Color {
    if luminance(behind) < 0.18 {
        Color::Rgb(255, 255, 255)
    } else {
        Color::Rgb(0, 0, 0)
    }
}

/// `c`, moved away from the background only as far as `min` needs.
fn with_contrast(c: Color, behind: (u8, u8, u8), min: f64) -> Color {
    let away = away_from(behind);
    (0..=100)
        .map(|step| lerp(c, away, step as f32 / 100.0))
        .find(|&moved| contrast(moved, behind) >= min)
        .unwrap_or(away)
}

/// The ramp a renderer should paint with. `None` means "flat series".
pub fn fade_ramp(base: Color, opts: ChartOpts) -> Option<Ramp> {
    if !opts.fade {
        return None;
    }
    if opts.terminal_palette {
        return Some(palette_ramp(base));
    }
    Some(magnitude_ramp(base, opts.bg))
}

/// Braille dot bit per sub-row, left then right column of a cell.
const BRAILLE_LEFT: [u8; 4] = [0x01, 0x02, 0x04, 0x40];
const BRAILLE_RIGHT: [u8; 4] = [0x08, 0x10, 0x20, 0x80];
const BRAILLE_BASE: u32 = 0x2800;

/// Braille area plot at two samples per cell column, growing up from the
/// bottom edge (`flip` grows down from the top: that is the mirrored half).
///
/// `samples` is oldest-first. Every cell is coloured by its own height, so
/// severity is pre-attentive: the spike is visible before the axis is read.
pub fn area_graph(
    buf: &mut Buffer,
    area: Rect,
    samples: &[u64],
    max: u64,
    ramp: &Ramp,
    flip: bool,
    baseline: bool,
) {
    area_graph_range(buf, area, samples, 0, max, ramp, flip, baseline);
}

/// [`area_graph`] over an explicit range, for a series with a physical domain
/// (RSRP lives between -140 and -44 dBm, and its floor is not zero).
#[allow(clippy::too_many_arguments)]
pub fn area_graph_range(
    buf: &mut Buffer,
    area: Rect,
    samples: &[u64],
    min: u64,
    max: u64,
    ramp: &Ramp,
    flip: bool,
    baseline: bool,
) {
    if area.width == 0 || area.height == 0 || max <= min {
        return;
    }
    let w = area.width as usize;
    let h = area.height as usize;
    let sub_h = h * 4;

    for cx in 0..w {
        let lh = sub_height(samples.get(cx * 2).copied().unwrap_or(min), min, max, sub_h);
        let rh = sub_height(
            samples.get(cx * 2 + 1).copied().unwrap_or(min),
            min,
            max,
            sub_h,
        );
        // A zero sample still draws its baseline dot: a gap reads as "no data"
        // when what happened was "no traffic".
        let (lh, rh) = if baseline {
            (lh.max(1), rh.max(1))
        } else if lh == 0 && rh == 0 {
            continue;
        } else {
            (lh, rh)
        };

        for cy in 0..h {
            let mut bits: u8 = 0;
            for s in 0..4 {
                let from_top = cy * 4 + s;
                let depth = if flip { from_top + 1 } else { sub_h - from_top };
                if lh >= depth {
                    bits |= BRAILLE_LEFT[s];
                }
                if rh >= depth {
                    bits |= BRAILLE_RIGHT[s];
                }
            }
            if bits == 0 {
                continue;
            }
            // The cell's vertical midpoint, measured along the direction of
            // growth, so both halves of a mirrored pair brighten as traffic
            // climbs.
            let mid = (cy * 4 + 2) as f32;
            let f = if flip {
                mid / sub_h as f32
            } else {
                (sub_h as f32 - mid) / sub_h as f32
            };
            let Some(ch) = char::from_u32(BRAILLE_BASE | bits as u32) else {
                continue;
            };
            if let Some(cell) = buf.cell_mut((area.x + cx as u16, area.y + cy as u16)) {
                cell.set_char(ch);
                cell.set_style(Style::default().fg(ramp.at(f)));
            }
        }
    }
}

/// Sub-cell height of `v` inside `min..=max`, rounded half up in integers.
fn sub_height(v: u64, min: u64, max: u64, sub_h: usize) -> usize {
    if max <= min {
        return 0;
    }
    let v = v.clamp(min, max) - min;
    let span = max - min;
    ((v as u128 * sub_h as u128 * 2 + span as u128) / (span as u128 * 2)) as usize
}

/// Draw `data` (oldest first) into `area`, one value per column bucket.
pub fn render_with_max(
    buf: &mut Buffer,
    area: Rect,
    data: &[u64],
    max: u64,
    bars: bool,
    base: Color,
    opts: ChartOpts,
) {
    if area.width == 0 || area.height == 0 {
        return;
    }
    let ramp = fade_ramp(base, opts).unwrap_or_else(|| Ramp::flat(base));
    if opts.ascii {
        render_ascii(buf, area, data, max, &ramp, opts, false);
    } else if bars {
        render_bars(buf, area, data, max, &ramp, opts);
    } else {
        let samples = two_per_column(data, area.width);
        area_graph(buf, area, &samples, max, &ramp, false, opts.baseline);
    }
}

/// Single-width fallback glyphs, bottom-anchored in the same order as the
/// eighths. A font that draws braille or blocks double-width still gets one
/// character per cell here, so a line cannot wrap in the middle of a plot.
const ASCII_LEVELS: [char; 9] = [' ', '.', ':', '-', '=', '+', '*', '#', '%'];

/// One ASCII glyph per cell, coloured by height. `flip` hangs it from the top
/// for the mirrored half.
fn render_ascii(
    buf: &mut Buffer,
    area: Rect,
    data: &[u64],
    max: u64,
    ramp: &Ramp,
    opts: ChartOpts,
    flip: bool,
) {
    if max == 0 || area.width == 0 || area.height == 0 {
        return;
    }
    let cell_w = area.width as usize;
    let cell_h = area.height as usize;
    let start = data.len().saturating_sub(cell_w);
    let samples = &data[start..];
    let offset = cell_w.saturating_sub(samples.len());
    for (i, &v) in samples.iter().enumerate() {
        let mut eighths = (v.min(max) as u128 * cell_h as u128 * 8 / max as u128) as usize;
        if opts.baseline {
            eighths = eighths.max(1);
        } else if eighths == 0 {
            continue;
        }
        let x = area.x + (offset + i) as u16;
        for cy in 0..cell_h {
            let from_bottom = if flip { cy } else { cell_h - 1 - cy };
            let level = eighths.saturating_sub(from_bottom * 8).min(8);
            let glyph = ASCII_LEVELS[level];
            if glyph == ' ' {
                continue;
            }
            let f = (from_bottom as f32 + 0.5) / cell_h as f32;
            if let Some(cell) = buf.cell_mut((x, area.y + cy as u16)) {
                cell.set_char(glyph);
                cell.set_style(Style::default().fg(ramp.at(f)));
            }
        }
    }
}

/// Bottom-anchored eighths, one glyph per cell, coloured by height.
fn render_bars(buf: &mut Buffer, area: Rect, data: &[u64], max: u64, ramp: &Ramp, opts: ChartOpts) {
    const GLYPHS: [char; 9] = [' ', '▁', '▂', '▃', '▄', '▅', '▆', '▇', '█'];
    if max == 0 {
        return;
    }
    let cell_w = area.width as usize;
    let cell_h = area.height as usize;
    let start = data.len().saturating_sub(cell_w);
    let samples = &data[start..];
    let offset = cell_w.saturating_sub(samples.len());
    for (i, &v) in samples.iter().enumerate() {
        let mut eighths = (v.min(max) as u128 * cell_h as u128 * 8 / max as u128) as usize;
        if opts.baseline {
            eighths = eighths.max(1);
        } else if eighths == 0 {
            continue;
        }
        let x = area.x + (offset + i) as u16;
        for cy in 0..cell_h {
            let from_bottom = cell_h - 1 - cy;
            let level = eighths.saturating_sub(from_bottom * 8).min(8);
            let glyph = GLYPHS[level];
            if glyph == ' ' {
                continue;
            }
            let f = (from_bottom as f32 + 0.5) / cell_h as f32;
            if let Some(cell) = buf.cell_mut((x, area.y + cy as u16)) {
                cell.set_char(glyph);
                cell.set_style(Style::default().fg(ramp.at(f)));
            }
        }
    }
}

/// Right-align `data` into exactly `2 * width` samples, zero-padding the left.
pub fn two_per_column(data: &[u64], width: u16) -> Vec<u64> {
    let want = width as usize * 2;
    let take = data.len().min(want);
    let mut out = vec![0u64; want - take];
    out.extend_from_slice(&data[data.len() - take..]);
    out
}

/// An in-text sparkline: the same renderer, converted back to `Line`s so it
/// slots into a `Paragraph` beside its label.
///
/// `bars` is a one-row contract: a block sparkline is a row of eighths, and
/// `height` is ignored for it. The dashboard passes 2 and the old tests pin
/// one line, so the distinction is kept here rather than at every call site.
#[allow(clippy::too_many_arguments)]
pub fn spark_lines(
    values: &VecDeque<f64>,
    width: usize,
    height: usize,
    bars: bool,
    base: Color,
    min: u64,
    max: u64,
    opts: ChartOpts,
) -> Vec<Line<'static>> {
    if width == 0 || height == 0 {
        return Vec::new();
    }
    let (min, max) = if max <= min {
        (min, min + 1)
    } else {
        (min, max)
    };
    let data: Vec<u64> = values
        .iter()
        .map(|v| (v.max(0.0) as u64).clamp(min, max) - min)
        .collect();
    let mut buf = Buffer::empty(Rect::new(0, 0, width as u16, height as u16));
    let area = buf.area;
    render_with_max(&mut buf, area, &data, max - min, bars, base, opts);
    (0..height as u16)
        .map(|y| {
            let spans: Vec<Span<'static>> = (0..width as u16)
                .map(|x| {
                    let cell = &buf[(x, y)];
                    // Untouched cells stay spaces. A braille blank (U+2800)
                    // would be invisible in most fonts, but a font without that
                    // glyph paints a fallback box in every empty cell of the
                    // plot, which reads as a stray character.
                    Span::styled(cell.symbol().to_string(), Style::default().fg(cell.fg))
                })
                .collect();
            Line::from(spans)
        })
        .collect()
}

/// [`spark_lines`] for a series with a signed physical domain (RSRP -140..-44
/// dBm, RSRQ -20..-3 dB, SINR -10..30 dB).
///
/// The values are shifted onto zero here, so the renderer keeps one unsigned
/// range while the caller keeps its float domain. Casting a negative domain
/// straight to `u64` (`(-44.0).max(1.0) as u64 == 1`) collapsed every signal
/// graph to a flat line, and the existing tests did not catch it because a
/// baseline dot still paints.
#[allow(clippy::too_many_arguments)]
pub fn spark_lines_scaled(
    values: &VecDeque<f64>,
    width: usize,
    height: usize,
    bars: bool,
    base: Color,
    min: f64,
    max: f64,
    opts: ChartOpts,
) -> Vec<Line<'static>> {
    let span = (max - min).max(1.0);
    let shifted: VecDeque<f64> = values.iter().map(|v| v - min).collect();
    spark_lines(
        &shifted,
        width,
        height,
        bars,
        base,
        0,
        span.round() as u64,
        opts,
    )
}

/// [`spark_lines`] with the ceiling derived from the data.
pub fn spark_auto(
    values: &VecDeque<f64>,
    width: usize,
    height: usize,
    bars: bool,
    base: Color,
    opts: ChartOpts,
) -> Vec<Line<'static>> {
    let max = values.iter().cloned().fold(0.0_f64, f64::max).max(1.0) as u64;
    spark_lines(values, width, height, bars, base, 0, max, opts)
}

/// Right-align `data` onto one time window of `slots` columns, taking the
/// maximum of the samples that land in each slot.
///
/// A sample stands for the whole interval it was measured over, not for one
/// instant: mapping only its timestamp leaves holes wherever the source is
/// coarser than a column, and the plot then reads as dropouts instead of a
/// steady series. Averaging is wrong for the same job: this is a spike hunt,
/// and a one-second stall disappears into the quiet seconds around it.
pub fn resample_to_window(
    data: &[u64],
    secs_per_sample: u64,
    window_secs: u64,
    slots: usize,
) -> Vec<u64> {
    let mut out = vec![0u64; slots];
    if slots == 0 || window_secs == 0 || data.is_empty() {
        return out;
    }
    let secs_per_sample = secs_per_sample.max(1);
    for (i, &v) in data.iter().enumerate() {
        let age = (data.len() - 1 - i) as u64 * secs_per_sample;
        if age >= window_secs {
            continue;
        }
        let newest = window_secs - age - 1;
        let oldest = newest.saturating_sub(secs_per_sample - 1);
        let to_slot =
            |from_left: u64| (from_left as u128 * slots as u128 / window_secs as u128) as usize;
        let (first, last) = (to_slot(oldest), to_slot(newest).min(slots - 1));
        for slot in out.iter_mut().take(last + 1).skip(first) {
            *slot = (*slot).max(v);
        }
    }
    out
}

/// Right-align `data` onto one time window of `slots` columns, keeping the
/// smallest sample that lands in each slot.
///
/// The companion of [`resample_to_window`] for a quality series: what matters
/// there is the dip, so the bucket takes the worst reading instead of the
/// biggest. `None` means no sample covered that slot, which must stay blank
/// rather than read as "no signal".
pub fn resample_min_to_window(
    data: &[u64],
    secs_per_sample: u64,
    window_secs: u64,
    slots: usize,
) -> Vec<Option<u64>> {
    let mut out = vec![None; slots];
    if slots == 0 || window_secs == 0 || data.is_empty() {
        return out;
    }
    let secs_per_sample = secs_per_sample.max(1);
    for (i, &v) in data.iter().enumerate() {
        let age = (data.len() - 1 - i) as u64 * secs_per_sample;
        if age >= window_secs {
            continue;
        }
        let newest = window_secs - age - 1;
        let oldest = newest.saturating_sub(secs_per_sample - 1);
        let to_slot =
            |from_left: u64| (from_left as u128 * slots as u128 / window_secs as u128) as usize;
        let (first, last) = (to_slot(oldest), to_slot(newest).min(slots - 1));
        for slot in out.iter_mut().take(last + 1).skip(first) {
            *slot = Some(match *slot {
                Some(current) => current.min(v),
                None => v,
            });
        }
    }
    out
}

/// A ceiling one outlier cannot dominate: the 95th percentile, raised to half
/// again above the median so a steady series does not sit at the ceiling.
pub fn robust_max(data: &[u64], percentile: f64) -> u64 {
    let mut vals: Vec<u64> = data.iter().copied().filter(|&v| v > 0).collect();
    if vals.is_empty() {
        return 1;
    }
    vals.sort_unstable();
    let idx = ((vals.len() - 1) as f64 * percentile).round() as usize;
    let p = vals[idx.min(vals.len() - 1)];
    let median = vals[vals.len() / 2];
    let headroom = median.saturating_add(median / 2).max(median + 1);
    p.max(headroom).max(1)
}

/// A shared-axis pair: rx growing up from the zero line, tx hanging below it.
/// The zero line is one row and belongs to rx, so the two halves cannot draw
/// two parallel baselines a row apart.
///
/// Returns the height of the rx half so the caller can label that row `0`
/// without recomputing the split and drifting from it.
#[allow(clippy::too_many_arguments)]
pub fn render_mirrored_scaled(
    buf: &mut Buffer,
    plot: Rect,
    rx: &[u64],
    tx: &[u64],
    rx_max: u64,
    tx_max: u64,
    bars: bool,
    rx_color: Color,
    tx_color: Color,
    opts: ChartOpts,
) -> u16 {
    if plot.width == 0 || plot.height == 0 {
        return 0;
    }
    let tx_h = plot.height / 2;
    let rx_h = plot.height - tx_h;
    render_half(
        buf,
        Rect {
            height: rx_h,
            ..plot
        },
        rx,
        rx_max,
        bars,
        rx_color,
        opts,
        false,
    );
    render_half(
        buf,
        Rect {
            y: plot.y + rx_h,
            height: tx_h,
            ..plot
        },
        tx,
        tx_max,
        bars,
        tx_color,
        // The zero row the rx half draws is this half's floor too.
        ChartOpts {
            baseline: false,
            ..opts
        },
        true,
    );
    rx_h
}

#[allow(clippy::too_many_arguments)]
fn render_half(
    buf: &mut Buffer,
    area: Rect,
    data: &[u64],
    max: u64,
    bars: bool,
    color: Color,
    opts: ChartOpts,
    flip: bool,
) {
    if area.width == 0 || area.height == 0 {
        return;
    }
    let ramp = fade_ramp(color, opts).unwrap_or_else(|| Ramp::flat(color));
    if opts.ascii {
        render_ascii(buf, area, data, max, &ramp, opts, flip);
    } else if bars {
        if flip {
            render_bars_flipped(buf, area, data, max, &ramp, opts);
        } else {
            render_bars(buf, area, data, max, &ramp, opts);
        }
    } else {
        // One sample owns one cell: both dot columns of the cell carry the same
        // value. Asking for two samples per cell instead stretched a short
        // history over the whole plot, so one reading painted a four-cell bar
        // and the bar width changed as the history grew.
        let pad = (area.width as usize).saturating_sub(data.len());
        let mut samples: Vec<u64> = vec![0; pad * 2];
        samples.extend(data.iter().flat_map(|&v| [v, v]));
        area_graph(buf, area, &samples, max, &ramp, flip, opts.baseline);
    }
}

/// Compress `data` into exactly `cells` buckets, keeping the largest sample in
/// each one. A bucket feeds one cell, so a spike can never widen past a cell
/// however long the history is.
pub fn bucket_max(data: &[u64], cells: usize) -> Vec<u64> {
    let mut out = vec![0u64; cells];
    if cells == 0 || data.is_empty() {
        return out;
    }
    for (i, &v) in data.iter().enumerate() {
        let slot = (i * cells / data.len()).min(cells - 1);
        out[slot] = out[slot].max(v);
    }
    out
}

/// The x-axis strip under a time-series plot: the window's start, its
/// midpoint, and `now`, spread across `width`.
pub fn time_axis(width: u16, secs: u64) -> String {
    let w = width as usize;
    let mut out = String::new();
    for (i, frac) in [0.0f32, 0.5, 1.0].iter().enumerate() {
        let text = match i {
            2 => "now".to_string(),
            _ => format!("-{}s", (secs as f32 * (1.0 - frac)).round() as u64),
        };
        let target = ((w.saturating_sub(text.chars().count())) as f32 * frac).round() as usize;
        while out.chars().count() < target {
            out.push(' ');
        }
        out.push_str(&text);
    }
    out
}

/// Top-anchored blocks, for the mirrored half of a `bars` graph.
///
/// Three fill levels, not eight: the bottom-anchored eighths (`▁`..`▇`) have no
/// top-anchored counterparts outside Symbols for Legacy Computing, whose font
/// coverage is exactly what `bars` exists to avoid depending on. A coarser
/// mirror beats a mirrored half made of tofu.
fn render_bars_flipped(
    buf: &mut Buffer,
    area: Rect,
    data: &[u64],
    max: u64,
    ramp: &Ramp,
    opts: ChartOpts,
) {
    // `▔` 1/8, `▀` 1/2, `█` full: all in Block Elements.
    const GLYPHS: [char; 3] = ['▔', '▀', '█'];
    if max == 0 {
        return;
    }
    let cell_w = area.width as usize;
    let cell_h = area.height as usize;
    let start = data.len().saturating_sub(cell_w);
    let samples = &data[start..];
    let offset = cell_w.saturating_sub(samples.len());
    for (i, &v) in samples.iter().enumerate() {
        let mut eighths = (v.min(max) as u128 * cell_h as u128 * 8 / max as u128) as usize;
        if opts.baseline {
            eighths = eighths.max(1);
        } else if eighths == 0 {
            continue;
        }
        let x = area.x + (offset + i) as u16;
        for cy in 0..cell_h {
            let level = eighths.saturating_sub(cy * 8).min(8);
            let glyph = match level {
                0 => continue,
                1..=3 => GLYPHS[0],
                4..=7 => GLYPHS[1],
                _ => GLYPHS[2],
            };
            let f = (cy as f32 + 0.5) / cell_h as f32;
            if let Some(cell) = buf.cell_mut((x, area.y + cy as u16)) {
                cell.set_char(glyph);
                cell.set_style(Style::default().fg(ramp.at(f)));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ramp_samples_endpoints_and_midpoint() {
        let r = Ramp::new(vec![
            Color::Rgb(0, 0, 0),
            Color::Rgb(100, 0, 0),
            Color::Rgb(200, 0, 0),
        ]);
        assert_eq!(r.at(0.0), Color::Rgb(0, 0, 0));
        assert_eq!(r.at(1.0), Color::Rgb(200, 0, 0));
        assert_eq!(r.at(0.5), Color::Rgb(100, 0, 0));
        assert_eq!(Ramp::flat(Color::Red).at(0.9), Color::Red);
    }

    #[test]
    fn magnitude_ramp_climbs_from_the_floor() {
        let r = magnitude_ramp(Color::Green, Color::Rgb(0x10, 0x10, 0x10));
        let floor = luminance_of(r.at(0.0));
        let mid = luminance_of(r.at(0.5));
        let top = luminance_of(r.at(1.0));
        assert!(floor < mid, "ramp must climb: {floor} !< {mid}");
        assert!(mid < top, "ramp must climb: {mid} !< {top}");
    }

    #[test]
    fn the_dimmest_stop_stays_readable_on_the_background() {
        let bg = Color::Rgb(0x10, 0x10, 0x10);
        for base in [
            Color::Green,
            Color::Cyan,
            Color::Yellow,
            Color::Rgb(0xC6, 0x28, 0x28),
        ] {
            let r = magnitude_ramp(base, bg);
            let c = contrast(r.at(0.0), (0x10, 0x10, 0x10));
            assert!(c >= 4.0, "floor contrast {c:.2} under 4.0 for {base:?}");
        }
    }

    #[test]
    fn the_palette_ramp_invents_no_colour() {
        let r = palette_ramp(Color::Green);
        assert_eq!(r.at(0.0), Color::Green);
        assert_eq!(r.at(1.0), Color::LightGreen);
    }

    #[test]
    fn a_full_sample_paints_the_whole_braille_cell() {
        let mut buf = Buffer::empty(Rect::new(0, 0, 1, 1));
        let area = buf.area;
        let r = Ramp::flat(Color::Green);
        area_graph(&mut buf, area, &[1, 1], 1, &r, false, true);
        assert_eq!(
            buf[(0, 0)].symbol(),
            "\u{28ff}",
            "full height is eight dots"
        );
    }

    #[test]
    fn a_zero_sample_still_draws_its_baseline() {
        let mut buf = Buffer::empty(Rect::new(0, 0, 1, 1));
        let area = buf.area;
        let r = Ramp::flat(Color::Green);
        area_graph(&mut buf, area, &[0, 0], 1, &r, false, true);
        assert_eq!(buf[(0, 0)].symbol(), "\u{28c0}", "bottom row of dots");
    }

    #[test]
    fn without_a_baseline_an_empty_column_stays_empty() {
        let mut buf = Buffer::empty(Rect::new(0, 0, 1, 1));
        let area = buf.area;
        let r = Ramp::flat(Color::Green);
        area_graph(&mut buf, area, &[0, 0], 1, &r, false, false);
        assert_eq!(buf[(0, 0)].symbol(), " ");
    }

    #[test]
    fn render_covers_the_area_and_right_aligns() {
        let mut buf = Buffer::empty(Rect::new(0, 0, 4, 2));
        let area = buf.area;
        render_with_max(
            &mut buf,
            area,
            &[10, 10],
            10,
            false,
            Color::Green,
            ChartOpts::default(),
        );
        // Two samples fill the right-most column only; the left three stay empty.
        assert_eq!(buf[(0, 0)].symbol(), " ");
        assert_eq!(buf[(3, 1)].symbol(), "\u{28ff}");
    }

    #[test]
    fn a_bars_sparkline_is_one_row_of_eighths() {
        let mut buf = Buffer::empty(Rect::new(0, 0, 3, 1));
        let area = buf.area;
        render_with_max(
            &mut buf,
            area,
            &[1, 4, 8],
            8,
            true,
            Color::Green,
            ChartOpts::default(),
        );
        let row: String = (0..3).map(|x| buf[(x, 0)].symbol().to_string()).collect();
        assert_eq!(row, "\u{2581}\u{2584}\u{2588}", "1/8, 4/8, 8/8");
    }

    #[test]
    fn spark_lines_match_the_requested_size() {
        let mut v: VecDeque<f64> = VecDeque::new();
        for i in 0..40 {
            v.push_back(i as f64);
        }
        let lines = spark_auto(&v, 20, 3, false, Color::Green, ChartOpts::default());
        assert_eq!(lines.len(), 3);
        for line in &lines {
            assert_eq!(line.width(), 20);
        }
    }

    #[test]
    fn the_ascii_style_is_single_width_and_fills_the_row() {
        let mut v: VecDeque<f64> = VecDeque::new();
        for i in 0..40 {
            v.push_back(i as f64);
        }
        let opts = ChartOpts {
            ascii: true,
            ..ChartOpts::default()
        };
        let lines = spark_auto(&v, 20, 3, false, Color::Green, opts);
        assert_eq!(lines.len(), 3);
        for line in &lines {
            let text: String = line.spans.iter().map(|s| s.content.clone()).collect();
            assert_eq!(text.chars().count(), 20, "one glyph per cell: {text:?}");
            assert!(text.is_ascii(), "every glyph is single width: {text:?}");
        }
    }

    #[test]
    fn the_ascii_mirror_paints_both_halves() {
        let mut buf = Buffer::empty(Rect::new(0, 0, 2, 3));
        let plot = buf.area;
        let opts = ChartOpts {
            ascii: true,
            ..ChartOpts::default()
        };
        render_mirrored_scaled(
            &mut buf,
            plot,
            &[9, 9],
            &[9, 9],
            9,
            9,
            false,
            Color::Green,
            Color::Cyan,
            opts,
        );
        assert_eq!(buf[(1, 0)].symbol(), "%", "rx grows up to the ceiling");
        assert_eq!(buf[(1, 2)].symbol(), "%", "tx hangs from the zero row");
    }

    #[test]
    fn a_short_history_paints_one_cell_per_sample() {
        // The plot used to be stretched over the whole width, so five samples in
        // a ten-cell panel painted a two-cell bar. One sample owns one cell now.
        let mut buf = Buffer::empty(Rect::new(0, 0, 10, 3));
        let plot = buf.area;
        let opts = ChartOpts {
            baseline: false,
            ..ChartOpts::default()
        };
        render_mirrored_scaled(
            &mut buf,
            plot,
            &[0, 0, 0, 0, 9],
            &[],
            9,
            9,
            false,
            Color::Green,
            Color::Cyan,
            opts,
        );
        let inked = (0..10).filter(|x| buf[(*x, 0)].symbol() != " ").count();
        assert_eq!(inked, 1, "one sample owns one cell, whatever the width");
        assert_ne!(buf[(9, 0)].symbol(), " ", "the newest sample sits right");
    }

    #[test]
    fn a_bucket_never_widens_a_spike() {
        let mut data = [1u64; 120];
        data[60] = 500;
        let out = bucket_max(&data, 95);
        assert_eq!(out.len(), 95, "one bucket per cell");
        assert_eq!(
            out.iter().filter(|v| **v == 500).count(),
            1,
            "the spike keeps exactly one cell: {out:?}"
        );
        assert!(out.iter().filter(|v| **v == 1).count() >= 94);
    }

    #[test]
    fn bars_fill_the_requested_height() {
        // A block sparkline used to be clamped to one row whatever the caller
        // asked for, so a three-row slot in the traffic and speed panels left
        // two rows empty. The renderer already stacks eighths; only the clamp
        // held it back.
        let mut v: VecDeque<f64> = VecDeque::new();
        for i in 0..10 {
            v.push_back(i as f64);
        }
        let lines = spark_auto(&v, 10, 3, true, Color::Green, ChartOpts::default());
        assert_eq!(lines.len(), 3, "bars use the height they are given");
        let painted = |row: usize| -> usize {
            lines[row]
                .spans
                .iter()
                .flat_map(|s| s.content.chars())
                .filter(|c| *c != ' ')
                .count()
        };
        assert!(
            painted(0) < painted(2),
            "the bar grows upward: top {} bottom {}",
            painted(0),
            painted(2)
        );
    }

    #[test]
    fn resample_takes_the_max_of_each_slot() {
        // Ten one-second samples over a ten-second window: one slot per two
        // seconds, so the first slot holds samples 1 and 2 and takes the
        // larger of them. The spike of 9 lands in the second slot.
        let data = [1u64, 2, 3, 9, 1, 1, 1, 1, 1, 1];
        let out = resample_to_window(&data, 1, 10, 5);
        assert_eq!(out, vec![2, 9, 1, 1, 1]);
    }

    #[test]
    fn a_coarse_source_fills_every_slot_it_spans() {
        // A five-second sample inside a sixty-second window must not leave the
        // plot reading as a comb of dropouts.
        let data = [7u64; 4];
        let out = resample_to_window(&data, 15, 60, 8);
        assert!(out.iter().all(|&v| v == 7), "every slot covered: {out:?}");
    }

    #[test]
    fn robust_max_keeps_one_spike_from_flatlining_the_plot() {
        let mut data = [2u64; 100];
        data[7] = 200;
        let m = robust_max(&data, 0.95);
        assert!(m > 2, "the body must still have room: {m}");
        assert!(m < 200, "one spike may not own the ceiling: {m}");
    }

    #[test]
    fn the_mirror_shares_one_zero_row() {
        let mut buf = Buffer::empty(Rect::new(0, 0, 2, 3));
        let plot = buf.area;
        let opts = ChartOpts::default();
        let rx_h = render_mirrored_scaled(
            &mut buf,
            plot,
            &[9, 9],
            &[9, 9],
            9,
            9,
            false,
            Color::Green,
            Color::Cyan,
            opts,
        );
        assert_eq!(rx_h, 2, "the odd row goes to the busier half");
        assert_ne!(buf[(1, 0)].symbol(), " ", "rx grows up from the zero row");
        assert_ne!(buf[(1, 1)].symbol(), " ", "the zero row is rx's floor");
        assert_ne!(buf[(1, 2)].symbol(), " ", "tx grows down from the same row");
    }

    #[test]
    fn an_axis_names_its_own_window() {
        let s = time_axis(20, 240);
        assert!(s.ends_with("now"), "{s}");
        assert!(s.starts_with("-240s"), "{s}");
        assert!(s.contains("-120s"), "the midpoint is named: {s}");
        assert_eq!(s.chars().count(), 20, "the strip fills exactly the width");
    }

    #[test]
    fn a_bars_tx_half_paints_downward() {
        let mut buf = Buffer::empty(Rect::new(0, 0, 2, 3));
        let plot = buf.area;
        render_mirrored_scaled(
            &mut buf,
            plot,
            &[9, 9],
            &[9, 9],
            9,
            9,
            true,
            Color::Green,
            Color::Cyan,
            ChartOpts::default(),
        );
        // `bars` sends BOTH halves through the block renderer, so the rx half
        // is a full block, not braille: a full sample fills its one row.
        assert_eq!(buf[(1, 0)].symbol(), "█", "rx is a bottom-anchored block");
        assert_eq!(buf[(1, 2)].symbol(), "█", "tx reaches the bottom edge");
    }

    #[test]
    fn a_light_panel_keeps_its_peak_contrast() {
        // paper's own background: the top of the ramp must move toward black,
        // not toward white, or the peak washes out (measured 2.5:1 before).
        let bg = (0xF5u8, 0xF5u8, 0xF2u8);
        let r = magnitude_ramp(Color::Rgb(0x00, 0x64, 0x28), Color::Rgb(0xF5, 0xF5, 0xF2));
        for f in [0.0f32, 0.5, 1.0] {
            let c = contrast(r.at(f), bg);
            assert!(c >= 4.0, "stop {f} under 4:1 on a light panel: {c:.2}");
        }
    }

    #[test]
    fn a_braille_sparkline_paints_its_right_edge() {
        let mut v: VecDeque<f64> = VecDeque::new();
        for i in 0..50 {
            v.push_back(i as f64);
        }
        let lines = spark_auto(&v, 20, 3, false, Color::Reset, ChartOpts::default());
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
        let ink = |c: char| c != ' ' && c != '\u{2800}';
        let painted = text.chars().filter(|c| ink(*c)).count();
        assert!(painted > 20, "expected filled cells, got {painted}");
        let last_col_painted =
            (0..3).any(|row| text.chars().nth(row * 20 + 19).map(ink).unwrap_or(false));
        assert!(last_col_painted, "right edge must show the newest value");
    }

    #[test]
    fn a_scaled_series_keeps_its_middle_height() {
        // -96 of a -140..-44 domain sits at 46%: seven of sixteen sub-rows, so
        // two of the four rows carry ink. Casting the negative domain to u64
        // (min 0, max 1) collapsed every sample onto the baseline row instead,
        // and a baseline dot still paints, so the count used to stay at 1.
        let mut v: VecDeque<f64> = VecDeque::new();
        for _ in 0..40 {
            v.push_back(-96.0);
        }
        let lines = spark_lines_scaled(
            &v,
            20,
            4,
            false,
            Color::Reset,
            -140.0,
            -44.0,
            ChartOpts::default(),
        );
        let painted_rows = lines
            .iter()
            .filter(|l| {
                l.spans
                    .iter()
                    .any(|s| s.content.chars().any(|c| c != ' ' && c != '\u{2800}'))
            })
            .count();
        assert_eq!(painted_rows, 2, "46% of the domain fills two of four rows");
    }

    #[test]
    fn a_single_scaled_sample_is_visible_at_the_edge() {
        let mut v: VecDeque<f64> = VecDeque::new();
        v.push_back(-96.0);
        let lines = spark_lines_scaled(
            &v,
            20,
            2,
            false,
            Color::Reset,
            -140.0,
            -44.0,
            ChartOpts::default(),
        );
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
            "the newest sample must paint the right edge"
        );
    }

    #[test]
    fn a_quality_window_keeps_the_worst_sample() {
        // Ten one-second samples over a twelve-second window: the dip of 1
        // must own its bucket, and every bucket a sample covers is filled.
        let mut data = [9u64; 10];
        data[3] = 1;
        let out = resample_min_to_window(&data, 1, 12, 4);
        assert_eq!(out[1], Some(1), "the dip owns its slot: {out:?}");
        assert!(out.iter().all(Option::is_some), "no gaps: {out:?}");
        assert_eq!(
            resample_min_to_window(&[5u64], 1, 10, 5).len(),
            5,
            "one sample still fills the requested width"
        );
    }

    #[test]
    fn every_shipped_theme_keeps_its_ramp_readable() {
        // The runtime assumes a dark background when the theme has none, and a
        // light one when it paints its own: both directions must clear 4:1 at
        // the floor, the body and the peak.
        let themes = [
            crate::theme::terminal(),
            crate::theme::dark(),
            crate::theme::ocean(),
            crate::theme::solarized(),
            crate::theme::dracula(),
            crate::theme::nord(),
            crate::theme::sky(),
            crate::theme::paper(),
        ];
        for t in themes {
            let behind = to_rgb(t.bg, ASSUMED_DARK_BG);
            let r = magnitude_ramp(t.ok, t.bg);
            for f in [0.0f32, 0.5, 1.0] {
                let c = contrast(r.at(f), behind);
                assert!(c >= MIN_RAMP_CONTRAST, "{} stop {f}: {c:.2}", t.id);
            }
        }
    }

    fn luminance_of(c: Color) -> f64 {
        match c {
            Color::Rgb(r, g, b) => luminance((r, g, b)),
            other => luminance(to_rgb(other, (0, 0, 0))),
        }
    }
}
