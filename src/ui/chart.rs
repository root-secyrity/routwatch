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

fn lerp_u8(a: u8, b: u8, t: f32) -> u8 {
    (a as f32 + (b as f32 - a as f32) * t).round().clamp(0.0, 255.0) as u8
}

fn rgb(c: Color) -> Option<(u8, u8, u8)> {
    match c {
        Color::Rgb(r, g, b) => Some((r, g, b)),
        _ => None,
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
}
