//! Shared chart rendering: one braille renderer, one ramp, one time window.
//!
//! Behaviour ported from the family's `graph` module (netwatch 0.40.0, MIT):
//! a cell is coloured by how high it sits in the plot, the rx/tx pair grows
//! away from a single zero row, and a column is the same moment in every
//! series because the samples are resampled onto one window first.
//!
//! No `App`, no `Theme`, no network: a `Buffer`, a `Rect`, plain samples and
//! colours in, pixels out.

use ratatui::style::Color;

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
    /// Draw a floor dot under a zero sample, so quiet reads as quiet.
    pub baseline: bool,
}

impl Default for ChartOpts {
    fn default() -> Self {
        Self {
            fade: true,
            bg: Color::Reset,
            terminal_palette: false,
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
    Ramp::new(vec![
        fade_color(base, bg, floor),
        fade_color(base, bg, floor + (1.0 - floor) * 0.46),
        base,
        lighten(base, 0.42),
        lighten(base, 0.76),
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
            Color::Rgb(
                lerp_u8(ar, br, f),
                lerp_u8(ag, bgc, f),
                lerp_u8(ab, bb, f),
            )
        }
        _ => a,
    }
}

pub fn lighten(c: Color, f: f32) -> Color {
    lerp(c, Color::Rgb(255, 255, 255), f)
}

fn lerp_u8(a: u8, b: u8, t: f32) -> u8 {
    (a as f32 + (b as f32 - a as f32) * t).round().clamp(0.0, 255.0) as u8
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

/// `c`, moved away from the background only as far as `min` needs.
fn with_contrast(c: Color, behind: (u8, u8, u8), min: f64) -> Color {
    let away = if luminance(behind) < 0.18 {
        Color::Rgb(255, 255, 255)
    } else {
        Color::Rgb(0, 0, 0)
    };
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
        for base in [Color::Green, Color::Cyan, Color::Yellow, Color::Rgb(0xC6, 0x28, 0x28)] {
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

    fn luminance_of(c: Color) -> f64 {
        match c {
            Color::Rgb(r, g, b) => luminance((r, g, b)),
            other => luminance(to_rgb(other, (0, 0, 0))),
        }
    }
}
