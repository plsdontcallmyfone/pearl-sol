//! The two-tone block wordmark, in opencode's own pixel style (its TUI logo: 4 cells per
//! letter, 1 cell between letters, 4 rows with the first for ascenders). Marker
//! characters as opencode draws them:
//!   `_` a blank cell on the shadow color, `^` an upper half block over the shadow,
//!   `~` an upper half block in the shadow color, anything else drawn in the ink color.
//! The shadow is the ink mixed 25% over the background (opencode `tint(bg, fg, 0.25)`).
//! Letters o, p, e, n, c, d and g are opencode's; a, r, t, s are drawn to match.

/// One letter: 4 rows of 4 cells.
pub fn glyph(c: char) -> [&'static str; 4] {
    match c {
        'a' => ["    ", "▀▀▀█", "█^^█", "▀▀▀▀"],
        'c' => ["    ", "█▀▀▀", "█___", "▀▀▀▀"],
        'd' => ["   ▄", "█▀▀█", "█__█", "▀▀▀▀"],
        'e' => ["    ", "█▀▀█", "█^^^", "▀▀▀▀"],
        'g' => ["    ", "█▀▀▀", "█_^█", "▀▀▀▀"],
        'n' => ["    ", "█▀▀▄", "█__█", "▀~~▀"],
        'o' => ["    ", "█▀▀█", "█__█", "▀▀▀▀"],
        'p' => ["    ", "█▀▀█", "█__█", "█▀▀▀"],
        'r' => ["    ", "█▀▀▀", "█   ", "▀   "],
        's' => ["    ", "█▀▀▀", "▀▀▀█", "▀▀▀▀"],
        't' => ["▄   ", "█▀▀▀", "█   ", "▀▀▀▀"],
        _ => ["    ", "    ", "    ", "    "],
    }
}

/// A word as 4 marker rows (letters 1 cell apart).
pub fn word(w: &str) -> [String; 4] {
    let mut rows: [String; 4] = Default::default();
    for (i, c) in w.chars().enumerate() {
        let g = glyph(c);
        for (r, row) in rows.iter_mut().enumerate() {
            if i > 0 {
                row.push(' ');
            }
            row.push_str(g[r]);
        }
    }
    rows
}

/// How one marker character is drawn: (glyph, fg is ink (else shadow), bg is shadow).
pub fn cell(m: char) -> (char, bool, bool) {
    match m {
        '_' => (' ', true, true),
        '^' => ('▀', true, true),
        '~' => ('▀', false, false),
        c => (c, true, false),
    }
}

/// Width in cells of `left` + 1 + `right` side by side.
pub fn width(left: &str, right: &str) -> usize {
    let w = |s: &str| if s.is_empty() { 0 } else { s.chars().count() * 5 - 1 };
    w(left) + 1 + w(right)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn opencode_letters_match_its_logo() {
        // opencode's logo.ts, left half "open" and right half "code"
        let open = word("open");
        assert_eq!(open[1], "█▀▀█ █▀▀█ █▀▀█ █▀▀▄");
        assert_eq!(open[2], "█__█ █__█ █^^^ █__█");
        assert_eq!(open[3], "▀▀▀▀ █▀▀▀ ▀▀▀▀ ▀~~▀");
        let code = word("code");
        assert_eq!(code[0], "             ▄     ");
        assert_eq!(code[1], "█▀▀▀ █▀▀█ █▀▀█ █▀▀█");
        assert_eq!(code[2], "█___ █__█ █__█ █^^^");
        assert_eq!(code[3], "▀▀▀▀ ▀▀▀▀ ▀▀▀▀ ▀▀▀▀");
    }

    #[test]
    fn cadre_agents_size() {
        let (l, r) = (word("cadre"), word("agents"));
        assert!(l.iter().all(|x| x.chars().count() == 24));
        assert!(r.iter().all(|x| x.chars().count() == 29));
        assert_eq!(width("cadre", "agents"), 54);
        // every letter is defined (no blank glyphs)
        for c in "cadreagents".chars() {
            assert_ne!(glyph(c)[1], "    ", "{c}");
        }
    }
}
