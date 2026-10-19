//! Product name, ticker and the ASCII wordmark, in ONE place.
//!
//! Name chosen by the owner 2026-10-18: Lode, ticker LODE. A
//! rename is a change to [`NAME`] and [`TICKER`] only: the wordmark is generated from
//! [`NAME`] with the block font below, the bracketed top-right mark from it too.
//! (The crate and binary are still called `pearl-cli` / `pearl`; they move with the
//! rename, see docs/CLI.md.)

/// Product name shown on screen.
pub const NAME: &str = "LODE";
/// Ticker of the bridged token (chain units 9 dp, SPL 6 dp).
pub const TICKER: &str = "LODE";
/// Second header line.
pub const TAGLINE: &str = "Useful-PoW agent chain + Solana bridge (c) 2026";
/// Beta warning under the wordmark.
pub const WARNING: &str = "****This project is a beta version on a local stack, use at your own risk.****";

/// Small bracketed mark for the top-right corner, e.g. `<placeholder>`.
pub fn bracket_mark() -> String {
    format!("<{}>", NAME.to_lowercase())
}

/// 5x5 block font. `#` = ink. Unknown characters render as a blank cell.
fn glyph(c: char) -> [&'static str; 5] {
    match c.to_ascii_uppercase() {
        'A' => [".###.", "#...#", "#####", "#...#", "#...#"],
        'B' => ["####.", "#...#", "####.", "#...#", "####."],
        'C' => [".####", "#....", "#....", "#....", ".####"],
        'D' => ["####.", "#...#", "#...#", "#...#", "####."],
        'E' => ["#####", "#....", "####.", "#....", "#####"],
        'F' => ["#####", "#....", "####.", "#....", "#...."],
        'G' => [".####", "#....", "#..##", "#...#", ".###."],
        'H' => ["#...#", "#...#", "#####", "#...#", "#...#"],
        'I' => ["#####", "..#..", "..#..", "..#..", "#####"],
        'J' => ["..###", "...#.", "...#.", "#..#.", ".##.."],
        'K' => ["#...#", "#..#.", "###..", "#..#.", "#...#"],
        'L' => ["#....", "#....", "#....", "#....", "#####"],
        'M' => ["#...#", "##.##", "#.#.#", "#...#", "#...#"],
        'N' => ["#...#", "##..#", "#.#.#", "#..##", "#...#"],
        'O' => [".###.", "#...#", "#...#", "#...#", ".###."],
        'P' => ["####.", "#...#", "####.", "#....", "#...."],
        'Q' => [".###.", "#...#", "#.#.#", "#..#.", ".##.#"],
        'R' => ["####.", "#...#", "####.", "#..#.", "#...#"],
        'S' => [".####", "#....", ".###.", "....#", "####."],
        'T' => ["#####", "..#..", "..#..", "..#..", "..#.."],
        'U' => ["#...#", "#...#", "#...#", "#...#", ".###."],
        'V' => ["#...#", "#...#", "#...#", ".#.#.", "..#.."],
        'W' => ["#...#", "#...#", "#.#.#", "##.##", "#...#"],
        'X' => ["#...#", ".#.#.", "..#..", ".#.#.", "#...#"],
        'Y' => ["#...#", ".#.#.", "..#..", "..#..", "..#.."],
        'Z' => ["#####", "...#.", "..#..", ".#...", "#####"],
        '0' => [".###.", "#..##", "#.#.#", "##..#", ".###."],
        '1' => ["..#..", ".##..", "..#..", "..#..", ".###."],
        '2' => ["####.", "....#", ".###.", "#....", "#####"],
        '3' => ["####.", "....#", ".###.", "....#", "####."],
        '4' => ["#...#", "#...#", "#####", "....#", "....#"],
        '5' => ["#####", "#....", "####.", "....#", "####."],
        '6' => [".###.", "#....", "####.", "#...#", ".###."],
        '7' => ["#####", "....#", "...#.", "..#..", "..#.."],
        '8' => [".###.", "#...#", ".###.", "#...#", ".###."],
        '9' => [".###.", "#...#", ".####", "....#", ".###."],
        '<' => ["...#.", "..#..", ".#...", "..#..", "...#."],
        '>' => [".#...", "..#..", "...#.", "..#..", ".#..."],
        '-' => [".....", ".....", "#####", ".....", "....."],
        '_' => [".....", ".....", ".....", ".....", "#####"],
        _ => [".....", ".....", ".....", ".....", "....."],
    }
}

/// Figlet-style block wordmark of `text`, one string per row. Each ink pixel is
/// `ink` (`"$$"` for the wide form, `"$"` for the narrow one); glyphs are one
/// pixel apart. Trailing spaces are trimmed.
pub fn wordmark_with(text: &str, ink: &str) -> Vec<String> {
    let blank = " ".repeat(ink.chars().count());
    (0..5)
        .map(|row| {
            let mut line = String::new();
            for (i, c) in text.chars().enumerate() {
                if i > 0 {
                    line.push_str(&blank);
                }
                for px in glyph(c)[row].chars() {
                    line.push_str(if px == '#' { ink } else { &blank });
                }
            }
            line.trim_end().to_string()
        })
        .collect()
}

/// The widest wordmark of [`NAME`] that fits in `width` columns: `$$` pixels, then
/// `$` pixels, else none (the header then shows the name as text only).
pub fn wordmark(width: usize) -> Vec<String> {
    for ink in ["$$", "$"] {
        let w = wordmark_with(NAME, ink);
        if w.iter().map(|l| l.chars().count()).max().unwrap_or(0) <= width {
            return w;
        }
    }
    vec![]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wordmark_shape() {
        let w = wordmark_with("AB", "$$");
        assert_eq!(w.len(), 5);
        // A = 5 px, gap 1 px, B = 5 px; 2 cols per px; row 0 of B ends in '.', trimmed.
        assert_eq!(w[0], "  $$$$$$    $$$$$$$$");
        assert_eq!(w[2], "$$$$$$$$$$  $$$$$$$$");
        let n = wordmark_with("AB", "$");
        assert_eq!(n[2], "##### ####".replace('#', "$"));
    }

    #[test]
    fn wordmark_fits_or_falls_back() {
        let wide = wordmark_with(NAME, "$$").iter().map(|l| l.len()).max().unwrap();
        assert_eq!(wordmark(wide), wordmark_with(NAME, "$$"));
        let narrow = wordmark(wide - 1);
        assert_eq!(narrow, wordmark_with(NAME, "$"));
        assert!(wordmark(5).is_empty());
        assert_eq!(bracket_mark(), format!("<{}>", NAME.to_lowercase()));
    }

    #[test]
    fn every_name_char_has_a_glyph() {
        // A rename to letters, digits, '-', '_' or spaces needs no font change.
        for c in NAME.chars() {
            assert!(c == ' ' || glyph(c).iter().any(|r| r.contains('#')), "no glyph for {c:?}");
        }
    }
}
