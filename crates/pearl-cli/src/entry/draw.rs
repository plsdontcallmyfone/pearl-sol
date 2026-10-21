//! Drawing primitives shared by every view of the Cadre app: the pen (theme colors in the
//! terminal's color depth), clipped text, fills, word wrap, and the two surfaces of the
//! entry style: a quiet card (blue left bar on the `#1e1e1e` panel, half-block bottom
//! edge, like the prompt box) and a selected row (the panel color with the blue bar, like
//! a "you" line in the conversation). Everything else sits on the terminal's own background.

use ratatui::buffer::Buffer;
use ratatui::layout::{Position, Rect};
use ratatui::style::{Modifier, Style};
use unicode_width::UnicodeWidthChar;

use super::theme::{self, Mode, BLUE, DIM, GREEN, MUTED, PANEL, RED, TEXT, YELLOW};
use crate::term::Tone;

#[derive(Clone, Copy)]
pub struct Pen {
    pub mode: Mode,
}

impl Pen {
    pub fn fg(&self, c: u32) -> Style {
        match self.mode {
            Mode::NoColor if c == MUTED || c == DIM || c == theme::ROBOT[2] || c == theme::ROBOT[3] => Style::default().add_modifier(Modifier::DIM),
            Mode::NoColor => Style::default(),
            m => Style::default().fg(m.color(c)),
        }
    }
    pub fn on(&self, fg: u32, bg: u32) -> Style {
        match self.mode {
            Mode::NoColor => self.fg(fg),
            m => self.fg(fg).bg(m.color(bg)),
        }
    }
    pub fn bg(&self, bg: u32) -> Style {
        match self.mode {
            Mode::NoColor => Style::default(),
            m => Style::default().bg(m.color(bg)),
        }
    }
    /// Text on a card (or a selected row): `on(fg, PANEL)`.
    pub fn card(&self, fg: u32) -> Style {
        self.on(fg, PANEL)
    }
}

/// Theme color of a CLI output tone.
pub fn tone(t: Tone) -> u32 {
    match t {
        Tone::Plain => TEXT,
        Tone::Dim => MUTED,
        Tone::Good => GREEN,
        Tone::Bad => RED,
        Tone::Warn => YELLOW,
    }
}

/// Write `s` at (x, y), clipped to `max_x`; returns the x after it.
pub fn put(buf: &mut Buffer, x: u16, y: u16, s: &str, st: Style, max_x: u16) -> u16 {
    let area = buf.area;
    if y >= area.bottom() || y < area.top() {
        return x;
    }
    let mut x = x;
    for ch in s.chars() {
        let w = ch.width().unwrap_or(0) as u16;
        if w == 0 {
            continue;
        }
        if x + w > max_x.min(area.right()) {
            break;
        }
        if let Some(c) = buf.cell_mut(Position::new(x, y)) {
            c.reset();
            c.set_char(ch).set_style(st);
        }
        x += w;
    }
    x
}

/// Write `s` ending at `right` (right-aligned), not left of `min_x`; returns its start.
pub fn put_right(buf: &mut Buffer, right: u16, y: u16, s: &str, st: Style, min_x: u16) -> u16 {
    let w = str_w(s);
    let x = right.saturating_sub(w).max(min_x);
    put(buf, x, y, s, st, right);
    x
}

pub fn fill(buf: &mut Buffer, r: Rect, st: Style) {
    let r = r.intersection(buf.area);
    for y in r.top()..r.bottom() {
        for x in r.left()..r.right() {
            if let Some(c) = buf.cell_mut(Position::new(x, y)) {
                c.reset();
                c.set_char(' ').set_style(st);
            }
        }
    }
}

pub fn str_w(s: &str) -> u16 {
    s.chars().map(|c| c.width().unwrap_or(0) as u16).sum()
}

/// `s` cut to `w` cells, with "..." when cut.
pub fn clip(s: &str, w: usize) -> String {
    let total: usize = s.chars().map(|c| c.width().unwrap_or(0)).sum();
    if total <= w {
        return s.to_string();
    }
    if w <= 3 {
        return ".".repeat(w);
    }
    let mut out = String::new();
    let mut n = 0;
    for c in s.chars() {
        let cw = c.width().unwrap_or(0);
        if n + cw > w - 3 {
            break;
        }
        out.push(c);
        n += cw;
    }
    out + "..."
}

/// Word wrap to `w` cells (long words are cut).
pub fn wrap(s: &str, w: usize) -> Vec<String> {
    let w = w.max(1);
    let mut out = vec![];
    for para in s.split('\n') {
        let mut line = String::new();
        let mut lw = 0usize;
        for word in para.split(' ') {
            let ww: usize = word.chars().map(|c| c.width().unwrap_or(0)).sum();
            if lw > 0 && lw + 1 + ww > w {
                out.push(std::mem::take(&mut line));
                lw = 0;
            }
            if lw > 0 {
                line.push(' ');
                lw += 1;
            }
            for c in word.chars() {
                let cw = c.width().unwrap_or(0);
                if lw + cw > w {
                    out.push(std::mem::take(&mut line));
                    lw = 0;
                }
                line.push(c);
                lw += cw;
            }
        }
        out.push(line);
    }
    out
}

/// A card: the blue bar down the left, the panel color behind, and the prompt box's
/// half-block bottom edge on the last row. Content goes from (x + 3, y + 1) to
/// (right - 2, bottom - 2). Returns the content rectangle.
pub fn card(buf: &mut Buffer, pen: &Pen, r: Rect) -> Rect {
    let r = r.intersection(buf.area);
    if r.width < 2 || r.height < 2 {
        return Rect::new(r.x, r.y, 0, 0);
    }
    let bar = pen.fg(BLUE);
    for y in r.y..r.bottom() - 1 {
        put(buf, r.x, y, "┃", bar, r.right());
    }
    put(buf, r.x, r.bottom() - 1, "╹", bar, r.right());
    fill(buf, Rect::new(r.x + 1, r.y, r.width - 1, r.height - 1), pen.bg(PANEL));
    if pen.mode != Mode::NoColor {
        for x in r.x + 1..r.right() {
            put(buf, x, r.bottom() - 1, "▀", pen.fg(PANEL), r.right());
        }
    }
    let pad = if r.width >= 30 { 2 } else { 1 };
    Rect::new(r.x + 1 + pad, r.y + 1, r.width.saturating_sub(1 + 2 * pad), r.height.saturating_sub(3))
}

/// A selected row: the panel color from `x` to `right` with the blue bar at `x`.
pub fn selected_row(buf: &mut Buffer, pen: &Pen, x: u16, y: u16, right: u16) {
    fill(buf, Rect::new(x + 1, y, right.saturating_sub(x + 1), 1), pen.bg(PANEL));
    put(buf, x, y, "┃", pen.fg(BLUE), right);
}

/// A label (muted) and a value (its color) on one card row, the value right-aligned.
pub fn kv(buf: &mut Buffer, pen: &Pen, inner: Rect, y: u16, label: &str, value: &str, color: u32) {
    if y >= inner.bottom() {
        return;
    }
    let right = inner.right();
    let (lw, vw) = (str_w(label), str_w(value));
    // the value is what matters: the label gives way first
    let label = if lw + 1 + vw > inner.width { clip(label, inner.width.saturating_sub(vw + 1).max(inner.width / 3) as usize) } else { label.to_string() };
    let lx = put(buf, inner.x, y, &label, pen.card(MUTED), right);
    let room = right.saturating_sub(lx + 1) as usize;
    let v = clip(value, room);
    put_right(buf, right, y, &v, pen.card(color), lx + 1);
}

/// A card's title row: the title in text color, bold.
pub fn title(buf: &mut Buffer, pen: &Pen, inner: Rect, s: &str) {
    put(buf, inner.x, inner.y, s, pen.card(TEXT).add_modifier(Modifier::BOLD), inner.right());
}

/// Braille spinner frame for `ms` since start.
pub fn spinner(ms: u128) -> &'static str {
    const S: [&str; 10] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];
    S[(ms / 80) as usize % S.len()]
}

/// "4 s ago" style age of a read.
pub fn ago(ms: u64) -> String {
    let s = ms / 1000;
    if s < 60 {
        format!("{s} s ago")
    } else if s < 3600 {
        format!("{} min ago", s / 60)
    } else {
        format!("{} h ago", s / 3600)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clip_and_wrap() {
        assert_eq!(clip("abcdef", 6), "abcdef");
        assert_eq!(clip("abcdefg", 6), "abc...");
        assert_eq!(clip("abc", 2), "..");
        assert_eq!(wrap("aaa bbb ccc", 7), vec!["aaa bbb", "ccc"]);
        assert_eq!(wrap("abcdefghij", 4), vec!["abcd", "efgh", "ij"]);
        assert_eq!(ago(4_200), "4 s ago");
        assert_eq!(ago(125_000), "2 min ago");
    }

    #[test]
    fn card_surface() {
        let mut b = Buffer::empty(Rect::new(0, 0, 30, 6));
        let pen = Pen { mode: Mode::TrueColor };
        let inner = card(&mut b, &pen, Rect::new(0, 0, 30, 6));
        assert_eq!(inner, Rect::new(3, 1, 25, 3));
        assert_eq!(b[(0, 0)].symbol(), "┃");
        assert_eq!(b[(0, 5)].symbol(), "╹");
        assert_eq!(b[(5, 5)].symbol(), "▀");
        assert_eq!(b[(5, 2)].bg, Mode::TrueColor.color(PANEL));
    }
}
