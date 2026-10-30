//! Color themes - the same system netwatch, syswatch and diskwatch use.
//!
//! A struct of named color slots, a `by_name` lookup with the family's aliases,
//! a fixed `THEME_NAMES` cycle order, and `terminal` as the default: it resolves
//! every slot through the terminal's own palette (ANSI / `Reset`), so a
//! system-wide theme (a terminal profile, pywal, matugen, a rice) carries over
//! and routwatch sits beside the other three tools instead of fighting them.
//!
//! Slot names are routwatch's own; the palettes are netwatch's, mapped:
//! `accent←brand`, `title←active_tab`, `fg←text_primary`, `dim←text_muted`,
//! `ok/warn/danger←status_good/status_warn/status_error`, `border←border`,
//! `selected←selection_bg`, `inverse←text_inverse`, `bg←bg`. Byte-identical
//! hex values are deliberate - the same theme must look the same in all four
//! tools - so do not "tidy" them.

use ratatui::style::Color;
use std::collections::HashMap;
use std::path::Path;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GraphStyle {
    Bars,
    Dots,
    Braille,
}

impl GraphStyle {
    pub fn parse(s: &str) -> GraphStyle {
        match s.to_ascii_lowercase().as_str() {
            "bars" | "bar" => GraphStyle::Bars,
            "braille" | "btop" => GraphStyle::Braille,
            _ => GraphStyle::Dots,
        }
    }

    pub fn name(&self) -> &'static str {
        match self {
            GraphStyle::Bars => "bars",
            GraphStyle::Dots => "dots",
            GraphStyle::Braille => "braille",
        }
    }

    pub fn next(&self) -> GraphStyle {
        match self {
            GraphStyle::Dots => GraphStyle::Bars,
            GraphStyle::Bars => GraphStyle::Braille,
            GraphStyle::Braille => GraphStyle::Dots,
        }
    }
}

#[derive(Debug, Clone)]
pub struct Theme {
    pub id: String,
    /// Panel fill. `Reset` means "never paint over the terminal".
    pub bg: Color,
    pub fg: Color,
    pub dim: Color,
    pub accent: Color,
    pub ok: Color,
    pub warn: Color,
    pub danger: Color,
    pub border: Color,
    pub selected: Color,
    pub title: Color,
    /// Text drawn **on** the accent (active tab, cursor cells) - netwatch's
    /// `text_inverse`. Never reuse `bg` for this: on `terminal` it is `Reset`.
    pub inverse: Color,
}

/// Cycle order, identical to the family's: `dark` first, `terminal` second, the
/// painted-panel variants last.
pub const THEME_NAMES: &[&str] = &[
    "dark",
    "terminal",
    "auto",
    "ocean",
    "solarized",
    "dracula",
    "nord",
    "sky",
    "paper",
];

/// The theme used when the user has not asked for another one.
pub const DEFAULT_THEME: &str = "terminal";

pub fn names() -> &'static [&'static str] {
    THEME_NAMES
}

/// Resolve a theme name the way the family does: case-insensitive, with the
/// aliases users actually type. An unknown name is not an error here - it falls
/// back to `dark`; the CLI validates with [`validate`] instead.
pub fn by_name(name: &str) -> Theme {
    match name.trim().to_lowercase().as_str() {
        // "system" and "ansi" are what users coming from other TUIs reach for.
        "terminal" | "system" | "ansi" => terminal(),
        // `auto` is not a palette: it resolves whatever the desktop published,
        // and falls back to `terminal` when there is nothing to read.
        "auto" => live_auto(),
        // netwatch's alias ladder: the old transparent "light" theme became
        // `paper`, and "minimal" was its interim name.
        "light" | "minimal" => paper(),
        "ocean" => ocean(),
        "solarized" => solarized(),
        "dracula" => dracula(),
        "nord" => nord(),
        "sky" => sky(),
        "paper" => paper(),
        _ => dark(),
    }
}

/// True when the name (or an alias of it) is one this build ships.
pub fn is_known(name: &str) -> bool {
    let n = name.trim().to_lowercase();
    THEME_NAMES.contains(&n.as_str())
        || matches!(n.as_str(), "system" | "ansi" | "light" | "minimal")
}

/// The `--theme` validator's error text, worded like netwatch's.
pub fn validate(name: &str) -> Result<(), String> {
    if is_known(name) {
        Ok(())
    } else {
        Err(format!("Invalid theme. Use: {}", THEME_NAMES.join(", ")))
    }
}

/// Next theme in the family cycle; unknown input starts the cycle over.
pub fn next_name(current: &str) -> &'static str {
    let i = THEME_NAMES
        .iter()
        .position(|n| *n == current.trim().to_lowercase())
        .unwrap_or(THEME_NAMES.len() - 1);
    THEME_NAMES[(i + 1) % THEME_NAMES.len()]
}

/// Relative luminance of an RGB colour; other slots have no measurable value.
fn luminance(c: Color) -> f64 {
    let (r, g, b) = match c {
        Color::Rgb(r, g, b) => (r, g, b),
        _ => return 0.0,
    };
    let ch = |v: u8| {
        let s = v as f64 / 255.0;
        if s <= 0.03928 {
            s / 12.92
        } else {
            ((s + 0.055) / 1.055).powf(2.4)
        }
    };
    0.2126 * ch(r) + 0.7152 * ch(g) + 0.0722 * ch(b)
}

/// WCAG contrast ratio between two opaque colours (1.0 … 21.0).
pub fn contrast(a: Color, b: Color) -> f64 {
    let (la, lb) = (luminance(a), luminance(b));
    let (hi, lo) = if la > lb { (la, lb) } else { (lb, la) };
    (hi + 0.05) / (lo + 0.05)
}

const fn rgb(r: u8, g: u8, b: u8) -> Color {
    Color::Rgb(r, g, b)
}

impl Theme {
    /// The spelling used across the app.
    pub fn get(id: &str) -> Theme {
        by_name(id)
    }

    /// `sky` and `paper` paint their own panel fill; everything else leaves the
    /// terminal's background alone.
    pub fn paints_bg(&self) -> bool {
        self.bg != Color::Reset
    }

    /// Quality colour for signal readings (3GPP-ish thresholds, same as the plan).
    pub fn rsrp_color(&self, v: f32) -> Color {
        if v >= -80.0 {
            self.ok
        } else if v >= -100.0 {
            self.warn
        } else {
            self.danger
        }
    }

    pub fn sinr_color(&self, v: f32) -> Color {
        if v >= 20.0 {
            self.ok
        } else if v >= 10.0 {
            self.warn
        } else {
            self.danger
        }
    }

    pub fn rsrq_color(&self, v: f32) -> Color {
        if v >= -10.0 {
            self.ok
        } else if v >= -15.0 {
            self.warn
        } else {
            self.danger
        }
    }

    pub fn rsrp_quality(&self, v: f32) -> &'static str {
        if v >= -80.0 {
            "good"
        } else if v >= -100.0 {
            "fair"
        } else {
            "poor"
        }
    }
}

/// Defers every slot to the terminal's palette: ANSI names and `Reset`, no RGB.
pub fn terminal() -> Theme {
    Theme {
        id: "terminal".into(),
        bg: Color::Reset,
        fg: Color::Reset,
        dim: Color::DarkGray,
        accent: Color::Cyan,
        ok: Color::Green,
        warn: Color::Yellow,
        danger: Color::Red,
        border: Color::DarkGray,
        // The only slot conventionally a neutral mid-grey in light and dark.
        selected: Color::Indexed(8),
        title: Color::Yellow,
        inverse: Color::Black,
    }
}

/// The family's original palette: ANSI colors on the terminal's background.
pub fn dark() -> Theme {
    Theme {
        id: "dark".into(),
        bg: Color::Reset,
        fg: Color::White,
        dim: Color::DarkGray,
        accent: Color::Cyan,
        ok: Color::Green,
        warn: Color::Yellow,
        danger: Color::Red,
        border: Color::DarkGray,
        selected: rgb(40, 40, 60),
        title: Color::Yellow,
        inverse: Color::Black,
    }
}

/// Terminal.app "Ocean": bright ANSI variants over the deep-blue profile.
pub fn ocean() -> Theme {
    let bright_white = rgb(0xFF, 0xFF, 0xFF);
    // bright_black (#818383) on this bg is below AA: use a lighter neutral.
    let muted = rgb(0xB5, 0xB6, 0xB7);
    Theme {
        id: "ocean".into(),
        bg: Color::Reset,
        fg: bright_white,
        dim: muted,
        accent: rgb(0x14, 0xF0, 0xF0),
        ok: rgb(0x31, 0xE7, 0x22),
        warn: rgb(0xEA, 0xEC, 0x23),
        danger: rgb(0xFC, 0x39, 0x1F),
        border: muted,
        selected: rgb(0x21, 0x6D, 0xFF),
        title: bright_white,
        inverse: rgb(0, 0, 0),
    }
}

pub fn solarized() -> Theme {
    let base01 = rgb(88, 110, 117);
    let muted = rgb(0x7d, 0x92, 0x96); // 4.6:1 on base03
    Theme {
        id: "solarized".into(),
        bg: Color::Reset,
        fg: rgb(131, 148, 150),
        dim: muted,
        accent: rgb(42, 161, 152),
        ok: rgb(133, 153, 0),
        warn: rgb(181, 137, 0),
        danger: rgb(220, 50, 47),
        border: base01,
        selected: rgb(7, 54, 66),
        title: rgb(181, 137, 0),
        inverse: rgb(0, 43, 54),
    }
}

pub fn dracula() -> Theme {
    Theme {
        id: "dracula".into(),
        bg: Color::Reset,
        fg: rgb(248, 248, 242),
        // Dracula's #6272A4 comment is 3.0:1; lightened to 4.5:1 for body text.
        dim: rgb(0x8b, 0x98, 0xbd),
        accent: rgb(189, 147, 249),
        ok: rgb(80, 250, 123),
        warn: rgb(241, 250, 140),
        danger: rgb(255, 85, 85),
        border: rgb(98, 114, 164),
        selected: rgb(68, 71, 90),
        title: rgb(255, 121, 198),
        inverse: rgb(40, 42, 54),
    }
}

pub fn nord() -> Theme {
    Theme {
        id: "nord".into(),
        bg: Color::Reset,
        fg: rgb(216, 222, 233),
        // nord3 is a background shade (1.7:1 as text); sit between nord3/nord4.
        dim: rgb(0x98, 0xa3, 0xb8),
        accent: rgb(136, 192, 208),
        ok: rgb(163, 190, 140),
        warn: rgb(235, 203, 139),
        danger: rgb(191, 97, 106),
        border: rgb(67, 76, 94),
        selected: rgb(59, 66, 82),
        title: rgb(143, 188, 187),
        inverse: rgb(46, 52, 64),
    }
}

/// Painted-panel variant of `ocean` (Terminal.app Ocean profile background).
pub fn sky() -> Theme {
    // Saturated red on this deep blue is 2.0:1 - lightened to 3.9:1.
    let danger = rgb(0xFF, 0xA9, 0x9C);
    let muted = rgb(0xD2, 0xD3, 0xD4);
    Theme {
        id: "sky".into(),
        bg: rgb(0x22, 0x4F, 0xBC),
        fg: rgb(0xFF, 0xFF, 0xFF),
        dim: muted,
        accent: rgb(0x14, 0xF0, 0xF0),
        ok: rgb(0x31, 0xE7, 0x22),
        warn: rgb(0xEA, 0xEC, 0x23),
        danger,
        border: muted,
        selected: rgb(0x21, 0x6D, 0xFF),
        title: rgb(0xFF, 0xFF, 0xFF),
        inverse: rgb(0, 0, 0),
    }
}

/// Off-white painted-panel variant, tuned to WCAG AA+ against `#f5f5f2`.
pub fn paper() -> Theme {
    Theme {
        id: "paper".into(),
        bg: rgb(0xF5, 0xF5, 0xF2),
        fg: rgb(0x0d, 0x0d, 0x0d),
        dim: rgb(0x4a, 0x4a, 0x4a),
        accent: rgb(0, 100, 160),
        ok: rgb(0x00, 0x64, 0x28),
        warn: rgb(0x78, 0x50, 0x00),
        danger: rgb(180, 30, 30),
        border: rgb(160, 160, 160),
        selected: rgb(220, 230, 240),
        title: rgb(0x90, 0x58, 0x00),
        inverse: rgb(255, 255, 255),
    }
}

/// Environment override for the `auto` theme: any TOML with flat colour keys.
pub const THEME_FILE_ENV: &str = "ROUTWATCH_THEME_FILE";

/// `auto`: follow the desktop's published palette, silently falling back to
/// `terminal` when there is nothing to follow. Not a palette of its own, which
/// is why its id stays `auto` (the config stores what the user chose, not what
/// it resolved to on this machine).
pub fn live_auto() -> Theme {
    let home = std::env::var_os("HOME").map(std::path::PathBuf::from);
    let explicit = std::env::var_os(THEME_FILE_ENV).map(std::path::PathBuf::from);
    resolve_auto(home.as_deref(), explicit.as_deref()).unwrap_or_else(terminal)
}

/// The ladder, in order: an explicit file wins, then Omarchy, then Ryoku. Pure:
/// every input is a path, so tests point each rung at a temp directory.
pub fn resolve_auto(home: Option<&Path>, explicit: Option<&Path>) -> Option<Theme> {
    if let Some(path) = explicit {
        if let Some(theme) = read_flat(path) {
            return Some(theme);
        }
    }
    let home = home?;
    // The desktop that is actually running wins. A Ryoku machine also carries an
    // Omarchy-compatible `colors.toml`, and that copy can be stale - on this
    // machine it still described a theme from a week earlier, so preferring it
    // silently produced the wrong palette.
    if let Some(theme) = read_ryoku(home) {
        return Some(theme);
    }
    let omarchy = home.join(".local/state/omarchy/current/theme/colors.toml");
    read_flat(&omarchy)
}

fn read_flat(path: &Path) -> Option<Theme> {
    let text = std::fs::read_to_string(path).ok()?;
    theme_from_flat(&parse_flat(&text))
}

/// `background = "#131313"` plus the aliases desktop themes actually ship
/// (`bg`, `color0`, `fg`, `color7`).
fn parse_flat(text: &str) -> HashMap<String, String> {
    let mut map = HashMap::new();
    for line in text.lines() {
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let key: String = key
            .chars()
            .filter(|c| !matches!(c, '"' | '\'' | ' ' | '\t'))
            .collect();
        if key.is_empty() || key.starts_with('#') {
            continue;
        }
        // `#` starts a comment, but every colour *is* a `#RRGGBB` value, so only
        // a hash after whitespace can be a comment.
        let raw = value.trim();
        let raw = match raw.find(" #") {
            Some(at) => &raw[..at],
            None => raw,
        };
        let value = raw.trim().trim_matches(|c| c == '"' || c == '\'');
        if !value.is_empty() {
            map.insert(key, value.to_string());
        }
    }
    for (long, short) in [
        ("background", "bg"),
        ("foreground", "fg"),
        ("selection_background", "selection"),
    ] {
        if !map.contains_key(long) {
            if let Some(value) = map.get(short).cloned() {
                map.insert(long.to_string(), value);
            }
        }
    }
    map
}

fn theme_from_flat(map: &HashMap<String, String>) -> Option<Theme> {
    let pick = |keys: &[&str]| {
        keys.iter()
            .find_map(|k| map.get(*k))
            .and_then(|v| parse_hex(v))
    };
    let bg = pick(&["background", "bg", "color0"])?;
    let fg = pick(&["foreground", "fg", "color7"])?;
    Some(Theme {
        id: "auto".into(),
        bg,
        fg,
        dim: pick(&["dark_foreground", "dark_fg", "color8"]).unwrap_or(fg),
        accent: pick(&["accent", "blue", "color4"]).unwrap_or(fg),
        ok: pick(&["green", "color2"]).unwrap_or(fg),
        warn: pick(&["yellow", "color3"]).unwrap_or(fg),
        danger: pick(&["red", "color1"]).unwrap_or(fg),
        border: pick(&["lighter_background", "lighter_bg", "color0"]).unwrap_or(bg),
        selected: pick(&["selection_background", "selection"]).unwrap_or(bg),
        title: pick(&["accent", "blue", "color4"]).unwrap_or(fg),
        inverse: pick(&["bright_foreground", "bright_fg"]).unwrap_or(bg),
        // `magenta` / `color5` are deliberately never consulted: no pink here.
    })
}

fn read_ryoku(home: &Path) -> Option<Theme> {
    let name = std::fs::read_to_string(home.join(".config/ryoku/current/theme.name")).ok()?;
    let name = name.trim().to_string();
    if name.is_empty() {
        return None;
    }
    let path = home
        .join(".local/share/ryoku/themes")
        .join(&name)
        .join("scheme.json");
    let text = std::fs::read_to_string(path).ok()?;
    let value: serde_json::Value = serde_json::from_str(&text).ok()?;
    let scheme = value.get("dark").or_else(|| value.get("light"))?;
    Some(from_ryoku(&name, scheme))
}

/// Ryoku's own scheme file: material-ish slot names plus the terminal palette.
/// `terminal.normal.magenta` is never read.
pub fn from_ryoku(_name: &str, scheme: &serde_json::Value) -> Theme {
    let pick = |key: &str| scheme.get(key).and_then(|v| v.as_str()).and_then(parse_hex);
    let terminal_slot = |slot: &str| {
        scheme
            .get("terminal")
            .and_then(|t| t.get("normal"))
            .and_then(|n| n.get(slot))
            .and_then(|v| v.as_str())
            .and_then(parse_hex)
    };
    let fallback = terminal();
    Theme {
        id: "auto".into(),
        bg: pick("mSurface").unwrap_or(fallback.bg),
        fg: pick("mOnSurface").unwrap_or(fallback.fg),
        dim: pick("mOutline").unwrap_or(fallback.dim),
        accent: pick("mPrimary").unwrap_or(fallback.accent),
        ok: terminal_slot("green").unwrap_or(fallback.ok),
        warn: pick("mTertiary").unwrap_or(fallback.warn),
        danger: pick("mError").unwrap_or(fallback.danger),
        border: pick("mSurfaceVariant").unwrap_or(fallback.border),
        selected: pick("mHover").unwrap_or(fallback.selected),
        title: pick("mPrimary").unwrap_or(fallback.title),
        inverse: pick("mOnPrimary").unwrap_or(fallback.inverse),
    }
}

fn parse_hex(text: &str) -> Option<Color> {
    let hex = text.trim().trim_start_matches('#');
    if hex.len() != 6 {
        return None;
    }
    let n = u32::from_str_radix(hex, 16).ok()?;
    Some(rgb((n >> 16) as u8, (n >> 8) as u8, n as u8))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The eight palettes this build ships. `auto` is a resolver, not a
    /// palette: it has no fixed colours, so it is not in any palette loop.
    fn palettes() -> impl Iterator<Item = &'static &'static str> {
        names().iter().filter(|name| **name != "auto")
    }

    #[test]
    fn every_builtin_loads_under_its_own_name() {
        for name in palettes() {
            assert_eq!(&Theme::get(name).id, name);
        }
        assert_eq!(palettes().count(), 8, "the family ships eight palettes");
        assert!(names().contains(&"auto"), "auto must be reachable by name");
    }

    #[test]
    fn unknown_theme_falls_back_to_dark_like_the_family() {
        assert_eq!(by_name("nonsense").id, "dark");
        assert_eq!(by_name("").id, "dark");
        assert_eq!(by_name("DARK").id, "dark", "lookup is case-insensitive");
    }

    #[test]
    fn aliases_resolve_to_the_canonical_theme() {
        for alias in ["terminal", "system", "ansi", "System", "ANSI"] {
            assert_eq!(by_name(alias).id, "terminal", "alias {alias} failed");
        }
        for alias in ["light", "minimal", "Light"] {
            assert_eq!(by_name(alias).id, "paper", "alias {alias} failed");
        }
    }

    #[test]
    fn validate_names_every_theme_netwatch_style() {
        assert!(validate("sky").is_ok());
        assert!(validate("system").is_ok());
        let err = validate("pink").unwrap_err();
        assert!(err.starts_with("Invalid theme. Use: "), "{err}");
        for name in THEME_NAMES {
            assert!(err.contains(name), "the error must list {name}");
        }
    }

    #[test]
    fn terminal_pins_no_rgb_and_paints_nothing() {
        // The whole contract of this theme: every slot resolves through the
        // terminal's own palette, so a Debug dump must contain no RGB at all.
        let t = terminal();
        let dump = format!("{t:?}");
        assert!(
            !dump.contains("Rgb"),
            "terminal theme must not pin RGB: {dump}"
        );
        assert_eq!(t.bg, Color::Reset);
        assert!(!t.paints_bg());
        assert_eq!(t.selected, Color::Indexed(8));
    }

    #[test]
    fn only_sky_and_paper_paint_a_background() {
        for name in palettes() {
            let t = by_name(name);
            assert_eq!(
                t.paints_bg(),
                *name == "sky" || *name == "paper",
                "{name} painting={}",
                t.paints_bg()
            );
        }
        assert_eq!(sky().bg, rgb(0x22, 0x4F, 0xBC));
        assert_eq!(paper().bg, rgb(0xF5, 0xF5, 0xF2));
    }

    #[test]
    fn cycle_visits_every_theme_and_returns_home() {
        let mut cur = DEFAULT_THEME;
        let mut seen = vec![cur];
        for _ in 0..THEME_NAMES.len() - 1 {
            cur = next_name(cur);
            seen.push(cur);
        }
        assert_eq!(
            next_name(cur),
            DEFAULT_THEME,
            "a full cycle lands where it began"
        );
        let mut sorted = seen.clone();
        sorted.sort_unstable();
        let mut expected = THEME_NAMES.to_vec();
        expected.sort_unstable();
        assert_eq!(sorted, expected);
    }

    #[test]
    fn every_rgb_theme_keeps_body_and_muted_text_legible() {
        // WCAG: body text 4.5:1, muted text 3:1. `terminal` is skipped - both
        // slots come from the user's palette. Themes that leave the background
        // alone are measured against a dark reference, like netwatch does.
        const DARK_REFERENCE: Color = rgb(0x1a, 0x1b, 0x26);
        for name in palettes() {
            let t = by_name(name);
            if t.id == "terminal" {
                continue;
            }
            // `dark` and `paper`/`sky` mix ANSI and RGB; only pure-RGB pairs
            // are measurable, which is what this test guards.
            if matches!(t.fg, Color::Rgb(..)) && matches!(t.dim, Color::Rgb(..)) {
                let bg = if t.paints_bg() { t.bg } else { DARK_REFERENCE };
                assert!(
                    contrast(t.fg, bg) >= 4.5,
                    "{name}: body text contrast {:.1}:1 below AA",
                    contrast(t.fg, bg)
                );
                assert!(
                    contrast(t.dim, bg) >= 3.0,
                    "{name}: muted text contrast {:.1}:1 below 3:1",
                    contrast(t.dim, bg)
                );
            }
        }
    }

    #[test]
    fn graph_style_cycles() {
        assert_eq!(GraphStyle::Dots.next(), GraphStyle::Bars);
        assert_eq!(GraphStyle::Bars.next(), GraphStyle::Braille);
        assert_eq!(GraphStyle::Braille.next(), GraphStyle::Dots);
        assert_eq!(GraphStyle::parse("btop"), GraphStyle::Braille);
    }

    const RYOKU_FIXTURE: &str = r##"{
      "dark": {
        "mPrimary": "#E63312",
        "mOnPrimary": "#0F0F0F",
        "mSecondary": "#1F4E9C",
        "mError": "#FF3B1F",
        "mSurface": "#131313",
        "mOnSurface": "#EFE9DD",
        "mHover": "#E63312",
        "mSurfaceVariant": "#1E1E1E",
        "mOutline": "#8A8578",
        "mTertiary": "#F2C230",
        "terminal": { "normal": { "green": "#5E8C4A", "magenta": "#8A8578" } }
      }
    }"##;

    const OMARCHY_FIXTURE: &str = r##"
background = "#131313"
foreground = "#EFE9DD"
accent = "#E63312"
color1 = "#E63312"
color2 = "#5E8C4A"
color3 = "#F2C230"
color5 = "#8A8578"
color7 = "#EFE9DD"
"##;

    fn temp_home(tag: &str) -> std::path::PathBuf {
        let dir =
            std::env::temp_dir().join(format!("routwatch-theme-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn auto_is_opt_in_and_terminal_stays_the_default() {
        assert_eq!(DEFAULT_THEME, "terminal");
        assert!(names().contains(&"auto"));
        assert!(validate("auto").is_ok(), "the CLI must accept --theme auto");
    }

    #[test]
    fn an_explicit_theme_file_wins_over_every_desktop_path() {
        let home = temp_home("explicit");
        let omarchy = home.join(".local/state/omarchy/current/theme");
        std::fs::create_dir_all(&omarchy).unwrap();
        std::fs::write(
            omarchy.join("colors.toml"),
            "background = \"#000000\"\nforeground = \"#0000FF\"\n",
        )
        .unwrap();
        let file = home.join("mine.toml");
        std::fs::write(&file, OMARCHY_FIXTURE).unwrap();

        let theme = resolve_auto(Some(&home), Some(&file)).expect("explicit file");
        assert_eq!(theme.accent, rgb(0xE6, 0x33, 0x12), "the file did not win");
        assert_eq!(theme.id, "auto");
        let _ = std::fs::remove_dir_all(&home);
    }

    #[test]
    fn the_omarchy_colors_file_maps_every_slot() {
        let home = temp_home("omarchy");
        let dir = home.join(".local/state/omarchy/current/theme");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("colors.toml"), OMARCHY_FIXTURE).unwrap();

        let theme = resolve_auto(Some(&home), None).expect("omarchy theme");
        assert_eq!(theme.bg, rgb(0x13, 0x13, 0x13));
        assert_eq!(theme.fg, rgb(0xEF, 0xE9, 0xDD));
        assert_eq!(theme.accent, rgb(0xE6, 0x33, 0x12));
        assert_eq!(theme.danger, rgb(0xE6, 0x33, 0x12));
        assert_eq!(theme.warn, rgb(0xF2, 0xC2, 0x30));
        let _ = std::fs::remove_dir_all(&home);
    }

    #[test]
    fn the_ryoku_scheme_maps_every_slot() {
        let home = temp_home("ryoku");
        std::fs::create_dir_all(home.join(".config/ryoku/current")).unwrap();
        std::fs::write(
            home.join(".config/ryoku/current/theme.name"),
            "fixture-triad",
        )
        .unwrap();
        let dir = home.join(".local/share/ryoku/themes/fixture-triad");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("scheme.json"), RYOKU_FIXTURE).unwrap();

        let theme = resolve_auto(Some(&home), None).expect("ryoku theme");
        assert_eq!(theme.accent, rgb(0xE6, 0x33, 0x12));
        assert_eq!(theme.bg, rgb(0x13, 0x13, 0x13));
        assert_eq!(theme.fg, rgb(0xEF, 0xE9, 0xDD));
        assert_eq!(theme.danger, rgb(0xFF, 0x3B, 0x1F));
        assert_eq!(theme.ok, rgb(0x5E, 0x8C, 0x4A));
        let _ = std::fs::remove_dir_all(&home);
    }

    #[test]
    fn a_running_ryoku_desktop_wins_over_a_stale_omarchy_directory() {
        // Both paths exist on a Ryoku box; the Omarchy copy is a compatibility
        // artefact and may lag behind the desktop's current theme.
        let home = temp_home("both");
        let omarchy = home.join(".local/state/omarchy/current/theme");
        std::fs::create_dir_all(&omarchy).unwrap();
        std::fs::write(
            omarchy.join("colors.toml"),
            "background = \"#181818\"\nforeground = \"#EAEAEA\"\naccent = \"#654321\"\n",
        )
        .unwrap();
        std::fs::create_dir_all(home.join(".config/ryoku/current")).unwrap();
        std::fs::write(
            home.join(".config/ryoku/current/theme.name"),
            "fixture-triad",
        )
        .unwrap();
        let dir = home.join(".local/share/ryoku/themes/fixture-triad");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("scheme.json"), RYOKU_FIXTURE).unwrap();

        let theme = resolve_auto(Some(&home), None).expect("ryoku theme");
        assert_eq!(
            theme.accent,
            rgb(0xE6, 0x33, 0x12),
            "a stale omarchy copy shadowed the running desktop"
        );
        let _ = std::fs::remove_dir_all(&home);
    }

    #[test]
    fn a_machine_with_no_desktop_theme_falls_back_to_terminal() {
        let home = temp_home("empty");
        assert!(resolve_auto(Some(&home), None).is_none());
        assert!(resolve_auto(None, None).is_none());
        let _ = std::fs::remove_dir_all(&home);
    }

    #[test]
    fn no_derived_palette_takes_the_magenta_slot() {
        let home = temp_home("magenta");
        let dir = home.join(".local/state/omarchy/current/theme");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("colors.toml"), OMARCHY_FIXTURE).unwrap();
        let theme = resolve_auto(Some(&home), None).unwrap();
        for slot in [
            theme.dim,
            theme.accent,
            theme.border,
            theme.title,
            theme.selected,
        ] {
            assert_ne!(slot, rgb(0x8A, 0x85, 0x78), "magenta leaked into a slot");
        }
        let _ = std::fs::remove_dir_all(&home);
    }

    #[test]
    fn every_derived_palette_keeps_text_readable() {
        let home = temp_home("contrast");
        let dir = home.join(".local/state/omarchy/current/theme");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("colors.toml"), OMARCHY_FIXTURE).unwrap();
        let derived = resolve_auto(Some(&home), None).expect("derived");

        for name in palettes() {
            let theme = by_name(name);
            if matches!(theme.fg, Color::Rgb(..)) && matches!(theme.bg, Color::Rgb(..)) {
                assert!(
                    contrast(theme.fg, theme.bg) >= 4.5,
                    "{name} body text below AA"
                );
            }
        }
        assert!(
            contrast(derived.fg, derived.bg) >= 4.5,
            "the derived palette is unreadable: {:.2}:1",
            contrast(derived.fg, derived.bg)
        );
        let _ = std::fs::remove_dir_all(&home);
    }
}
