//! Colors of the entry screen: a dark theme (background, element, muted text, text,
//! blue), checked against the owner's reference screenshot.
//! Truecolor when the terminal says so (COLORTERM truecolor / 24bit), else the nearest
//! xterm 256-color index; NO_COLOR (set and not empty) drops every color.

use ratatui::style::Color;

/// Background.
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
/// Readable secondary text on the panel: palette descriptions, esc, the filter hint and the
/// prompt placeholder (owner 2026-10-21 "hard to read"): about 7.6:1 on PANEL, where muted
/// was about 4.3:1 and dim about 2.6:1.
pub const SUBTLE: u32 = 0xa8a8a8;
/// Robot shades, dense to light (darkStep12, darkStep11, darkStep8, darkStep7).
pub const ROBOT: [u32; 4] = [0xeeeeee, 0x808080, 0x606060, 0x484848];
/// Errors (darkRed) and success (darkGreen), for answers and DM results.
pub const RED: u32 = 0xe06c75;
pub const GREEN: u32 = 0x7fd88f;
/// Pending / in flight (darkYellow).
pub const YELLOW: u32 = 0xe5c07b;
/// Selected row in the command palette (darkStep9, primary).
pub const PRIMARY: u32 = 0xfab283;

/// `tint(base, overlay, alpha)`: `overlay` mixed over `base` at `alpha`.
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
        let v = |k: &str| std::env::var(k).ok();
        Mode::from_env(v("NO_COLOR").as_deref(), v("CADRE_COLOR").as_deref(), v("COLORTERM").as_deref(), v("TERM").as_deref(), v("TERM_PROGRAM").as_deref())
    }

    /// NO_COLOR wins; then CADRE_COLOR=truecolor|256|none; then COLORTERM=truecolor|24bit.
    /// Otherwise truecolor, because ssh does not forward COLORTERM and nearly every
    /// terminal in use takes 24-bit color; 256 colors only for terminals that clearly
    /// cannot (the Linux console, vt100-style, dumb, macOS Terminal.app).
    pub fn from_env(no_color: Option<&str>, cadre_color: Option<&str>, colorterm: Option<&str>, term: Option<&str>, term_program: Option<&str>) -> Mode {
        if no_color.is_some_and(|v| !v.is_empty()) {
            return Mode::NoColor;
        }
        match cadre_color.unwrap_or("").to_ascii_lowercase().as_str() {
            "truecolor" | "24bit" | "rgb" => return Mode::TrueColor,
            "256" | "ansi256" => return Mode::Ansi256,
            "none" | "off" | "0" | "no" => return Mode::NoColor,
            _ => {}
        }
        let ct = colorterm.unwrap_or("").to_ascii_lowercase();
        if ct == "truecolor" || ct == "24bit" {
            return Mode::TrueColor;
        }
        let t = term.unwrap_or("").to_ascii_lowercase();
        let limited = matches!(t.as_str(), "linux" | "dumb" | "ansi" | "cons25" | "xterm-color" | "xterm-16color") || t.starts_with("vt");
        if limited || term_program == Some("Apple_Terminal") {
            Mode::Ansi256
        } else {
            Mode::TrueColor
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

/// xterm-256 index for a color. Neutral grays (channels within 8 of each other) use only
/// the 24-step gray ramp 232-255, never the 16 system colors or the color cube, whose
/// entries many terminal themes tint; other colors take the nearest cube cell.
pub fn ansi256(rgb: u32) -> u8 {
    let (r, g, b) = ((rgb >> 16) as i32 & 0xff, (rgb >> 8) as i32 & 0xff, rgb as i32 & 0xff);
    if r.max(g).max(b) - r.min(g).min(b) <= 8 {
        let avg = (r + g + b) / 3;
        // ramp entry i is 8 + 10 i (8..=238)
        return 232 + (((avg - 8).max(0) + 5) / 10).min(23) as u8;
    }
    let lv = [0, 95, 135, 175, 215, 255];
    let near = |v: i32| (0..6).min_by_key(|&i| (lv[i] - v).abs()).unwrap();
    16 + (36 * near(r) + 6 * near(g) + near(b)) as u8
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shadows_mix_a_quarter() {
        // the logo shadow is 25% of the ink over the background
        assert_eq!(tint(BG, MUTED, 0.25), 0x282828);
        assert_eq!(tint(BG, TEXT, 0.25), 0x434343);
    }

    #[test]
    fn color_modes() {
        let m = |nc, cc, ct, t, tp| Mode::from_env(nc, cc, ct, t, tp);
        assert_eq!(m(Some("1"), None, Some("truecolor"), None, None), Mode::NoColor);
        assert_eq!(m(Some(""), None, Some("truecolor"), None, None), Mode::TrueColor);
        assert_eq!(m(None, None, Some("24bit"), None, None), Mode::TrueColor);
        // over ssh: TERM=xterm-256color and no COLORTERM is truecolor
        assert_eq!(m(None, None, None, Some("xterm-256color"), None), Mode::TrueColor);
        // terminals that cannot, and the override both ways
        assert_eq!(m(None, None, None, Some("linux"), None), Mode::Ansi256);
        assert_eq!(m(None, None, None, Some("vt100"), None), Mode::Ansi256);
        assert_eq!(m(None, None, None, Some("xterm-256color"), Some("Apple_Terminal")), Mode::Ansi256);
        assert_eq!(m(None, Some("256"), Some("truecolor"), None, None), Mode::Ansi256);
        assert_eq!(m(None, Some("truecolor"), None, Some("linux"), None), Mode::TrueColor);
        assert_eq!(m(None, Some("none"), None, None, None), Mode::NoColor);
        assert_eq!(Mode::TrueColor.color(BLUE), Color::Rgb(0x5c, 0x9c, 0xf5));
        assert_eq!(Mode::NoColor.color(BLUE), Color::Reset);
    }

    #[test]
    fn ansi256_fallback() {
        assert_eq!(ansi256(0x808080), 244);
        assert_eq!(ansi256(0xeeeeee), 255);
        assert_eq!(ansi256(0x0a0a0a), 232);
        assert_eq!(ansi256(0x1e1e1e), 234);
        assert_eq!(ansi256(0x282828), 235);
        assert_eq!(ansi256(0x434343), 238);
        assert_eq!(ansi256(0x000000), 232);
        assert_eq!(ansi256(0xffffff), 255);
        // every theme gray lands on the gray ramp
        for g in [super::BG, super::PANEL, super::MUTED, super::TEXT, super::DIM, 0x282828, 0x434343, 0x484848] {
            assert!(ansi256(g) >= 232, "{g:06x}");
        }
        assert_eq!(ansi256(0x5c9cf5), 75);
    }
}
