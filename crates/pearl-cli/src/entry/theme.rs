//! Colors of the entry screen: opencode's default dark theme (packages/tui theme
//! "opencode": darkStep1 background, darkStep3 element, darkStep11 muted text, darkStep12
//! text, darkSecondary blue), checked against the owner's reference screenshot.
//! Truecolor when the terminal says so (COLORTERM truecolor / 24bit), else the nearest
//! xterm 256-color index; NO_COLOR (set and not empty) drops every color.

use ratatui::style::Color;

/// Background (opencode darkStep1).
pub const BG: u32 = 0x0a0a0a;
/// Prompt panel (darkStep3, backgroundElement).
pub const PANEL: u32 = 0x1e1e1e;
/// Muted text: "cadre", placeholder, hint words, footer (darkStep11, textMuted).
pub const MUTED: u32 = 0x808080;
/// Main text: "agents", typed text, keys in the hints, model name (darkStep12, text).
pub const TEXT: u32 = 0xeeeeee;
/// The agent color: the left bar and the mode label (darkSecondary).
pub const BLUE: u32 = 0x5c9cf5;
/// Dim text: the prompt placeholder and secondary lines (darkStep8). The reference
/// screenshot shows the placeholder a step below muted: measured at its scale it peaks
/// under the muted footer text although it sits on the lighter panel.
pub const DIM: u32 = 0x606060;
/// Robot shades, dense to light (darkStep12, darkStep11, darkStep8, darkStep7).
pub const ROBOT: [u32; 4] = [0xeeeeee, 0x808080, 0x606060, 0x484848];
/// Errors (darkRed) and success (darkGreen), for answers and DM results.
pub const RED: u32 = 0xe06c75;
pub const GREEN: u32 = 0x7fd88f;
/// Selected row in the command palette (darkStep9, primary).
pub const PRIMARY: u32 = 0xfab283;

/// opencode `tint(base, overlay, alpha)`.
pub fn tint(base: u32, over: u32, alpha: f64) -> u32 {
    let ch = |s: u32| ((base >> s) & 0xff) as f64 + ((((over >> s) & 0xff) as f64) - (((base >> s) & 0xff) as f64)) * alpha;
    let r = |s| ch(s).round() as u32;
    (r(16) << 16) | (r(8) << 8) | r(0)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    TrueColor,
    Ansi256,
    NoColor,
}

impl Mode {
    pub fn detect() -> Mode {
        Mode::from_env(std::env::var("NO_COLOR").ok().as_deref(), std::env::var("COLORTERM").ok().as_deref(), std::env::var("TERM").ok().as_deref())
    }

    pub fn from_env(no_color: Option<&str>, colorterm: Option<&str>, term: Option<&str>) -> Mode {
        if no_color.is_some_and(|v| !v.is_empty()) {
            return Mode::NoColor;
        }
        let ct = colorterm.unwrap_or("").to_ascii_lowercase();
        let t = term.unwrap_or("").to_ascii_lowercase();
        if ct == "truecolor" || ct == "24bit" || t.contains("truecolor") || t.contains("24bit") || t.contains("direct") {
            Mode::TrueColor
        } else {
            Mode::Ansi256
        }
    }

    pub fn color(self, rgb: u32) -> Color {
        match self {
            Mode::TrueColor => Color::Rgb((rgb >> 16) as u8, (rgb >> 8) as u8, rgb as u8),
            Mode::Ansi256 => Color::Indexed(ansi256(rgb)),
            Mode::NoColor => Color::Reset,
        }
    }
}

/// Nearest xterm-256 color: the 6x6x6 cube or the 24-step gray ramp, whichever is closer.
pub fn ansi256(rgb: u32) -> u8 {
    let (r, g, b) = ((rgb >> 16) as i32 & 0xff, (rgb >> 8) as i32 & 0xff, rgb as i32 & 0xff);
    let lv = [0, 95, 135, 175, 215, 255];
    let near = |v: i32| (0..6).min_by_key(|&i| (lv[i] - v).abs()).unwrap();
    let (ci, cj, ck) = (near(r), near(g), near(b));
    let cube = (lv[ci], lv[cj], lv[ck]);
    let avg = (r + g + b) / 3;
    let gi = ((avg - 8).max(0) / 10).min(23);
    let gv = 8 + gi * 10;
    let d = |a: (i32, i32, i32)| (a.0 - r).pow(2) + (a.1 - g).pow(2) + (a.2 - b).pow(2);
    if d((gv, gv, gv)) < d(cube) {
        232 + gi as u8
    } else {
        16 + (36 * ci + 6 * cj + ck) as u8
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shadows_are_opencodes() {
        // the logo shadow is 25% of the ink over the background
        assert_eq!(tint(BG, MUTED, 0.25), 0x282828);
        assert_eq!(tint(BG, TEXT, 0.25), 0x434343);
    }

    #[test]
    fn color_modes() {
        assert_eq!(Mode::from_env(Some("1"), Some("truecolor"), None), Mode::NoColor);
        assert_eq!(Mode::from_env(Some(""), Some("truecolor"), None), Mode::TrueColor);
        assert_eq!(Mode::from_env(None, Some("24bit"), None), Mode::TrueColor);
        assert_eq!(Mode::from_env(None, None, Some("xterm-256color")), Mode::Ansi256);
        assert_eq!(Mode::TrueColor.color(BLUE), Color::Rgb(0x5c, 0x9c, 0xf5));
        assert_eq!(Mode::NoColor.color(BLUE), Color::Reset);
    }

    #[test]
    fn ansi256_fallback() {
        assert_eq!(ansi256(0x808080), 244);
        assert_eq!(ansi256(0xeeeeee), 255);
        assert_eq!(ansi256(0x0a0a0a), 232);
        assert_eq!(ansi256(0x1e1e1e), 234);
        assert_eq!(ansi256(0x5c9cf5), 75);
    }
}
