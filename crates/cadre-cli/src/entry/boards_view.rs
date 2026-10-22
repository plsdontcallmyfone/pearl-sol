//! The Boards view of the Cadre app (view 10, key 0; docs/CLI.md "Boards"): every leaderboard
//! cadre-api ranks from chain data (/v1/leaderboards), one board at a time. Tab or left/right
//! picks the board, w cycles the window (24h, 7d, all), h the side (organic, house, all). The
//! value is organic money only; house money sits in its own column and is never inside it.
//! The view reads only while it is open (the poller's Boards source, every 30 s).

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Modifier;
use serde_json::Value;

use super::draw::{self, clip, put, put_right, str_w, Pen};
use super::theme::{BLUE, DIM, GREEN, MUTED, RED, TEXT};
use super::views::{blit, canvas, max_scroll};
use crate::boards::{foot, n, row_cells, SIDES, WINDOWS};

#[derive(Debug)]
pub struct BoardsState {
    pub data: Option<Result<Value, String>>,
    /// index into the boards the API returned
    pub board: usize,
    pub window: usize,
    pub side: usize,
    pub scroll: u16,
}

impl Default for BoardsState {
    /// Opens on 7d, organic (the same defaults as `cadre top`).
    fn default() -> Self {
        BoardsState { data: None, board: 0, window: 1, side: 0, scroll: 0 }
    }
}

impl BoardsState {
    pub fn window(&self) -> &'static str {
        WINDOWS[self.window % WINDOWS.len()]
    }
    pub fn side(&self) -> &'static str {
        SIDES[self.side % SIDES.len()]
    }
    fn count(&self) -> usize {
        match &self.data {
            Some(Ok(v)) => v["boards"].as_array().map(|b| b.len()).unwrap_or(0),
            _ => 0,
        }
    }
    pub fn next_board(&mut self, d: i32) {
        let c = self.count().max(1) as i32;
        self.board = (((self.board as i32 + d) % c + c) % c) as usize;
        self.scroll = 0;
    }
    pub fn cycle_window(&mut self) {
        self.window = (self.window + 1) % WINDOWS.len();
    }
    pub fn cycle_side(&mut self) {
        self.side = (self.side + 1) % SIDES.len();
    }
}

/// window and side as poller codes (atomics), back to strings.
pub fn codes(w: u8, s: u8) -> (&'static str, &'static str) {
    (WINDOWS[w as usize % WINDOWS.len()], SIDES[s as usize % SIDES.len()])
}

/// One read: every board, top 25 rows, for the window and side.
pub fn fetch(api: Option<&str>, window: &str, side: &str) -> Result<Value, String> {
    let a = api.ok_or("no cadre-api on this network")?;
    super::net::get(&format!("{a}/leaderboards?window={window}&side={side}&limit=25")).map_err(|e| format!("{e:#}"))
}

fn s<'a>(v: &'a Value, k: &str) -> &'a str {
    v.get(k).and_then(|x| x.as_str()).unwrap_or("")
}

/// The board strip: every board title, the selected one marked.
pub fn strip(data: &Value, sel: usize) -> Vec<(String, bool)> {
    data["boards"].as_array().into_iter().flatten().enumerate().map(|(i, b)| (s(b, "title").to_string(), i == sel)).collect()
}

pub fn board_canvas(pen: &Pen, w: u16, b: &Value) -> Buffer {
    let rows: Vec<Vec<String>> = b["rows"].as_array().into_iter().flatten().map(|r| row_cells(b, r).to_vec()).collect();
    let colors: Vec<u32> = b["rows"].as_array().into_iter().flatten().map(|r| if n(&r["value"]) < 0 { RED } else { GREEN }).collect();
    let h = rows.len().max(1) as u16 + 6;
    let mut c = canvas(w, h + 6);
    super::trade::table_card(&mut c, pen, Rect::new(0, 0, w, h), s(b, "title"), &format!("{}  ·  {}", s(b, "window"), s(b, "side")), &["#", "WHO", "VALUE", "HOUSE", "SAMPLE", "PAYERS"], &rows, &colors, "no row reaches the minimum sample in this window");
    let lines = [(format!("Ranks: {}", s(b, "metric")), MUTED), (format!("Rule: {}", s(b, "rule")), DIM), (foot(b), DIM)];
    for (i, (t, col)) in lines.iter().enumerate() {
        put(&mut c, 1, h + 1 + i as u16, &clip(t, w.saturating_sub(2) as usize), pen.fg(*col), w);
    }
    c
}

pub fn draw_boards(buf: &mut Buffer, pen: &Pen, r: Rect, st: &BoardsState, ms: u128, loading: bool) {
    let x = put(buf, r.x, r.y, "Boards", pen.fg(TEXT).add_modifier(Modifier::BOLD), r.right());
    let x = put(buf, x + 2, r.y, &format!("window {}  ·  side {}", st.window(), st.side()), pen.fg(MUTED), r.right());
    if loading {
        put_right(buf, r.right(), r.y, &format!("{} reading", draw::spinner(ms)), pen.fg(DIM), x + 2);
    }
    let data = match &st.data {
        None => {
            put(buf, r.x + 2, r.y + 2, &format!("{} reading the leaderboards from cadre-api", draw::spinner(ms)), pen.fg(MUTED), r.right());
            return;
        }
        Some(Err(e)) => {
            put(buf, r.x, r.y + 2, &clip(&format!("leaderboards unavailable: {e}"), r.width as usize), pen.fg(RED), r.right());
            return;
        }
        Some(Ok(v)) => v,
    };
    // the board strip (wraps onto a second row when narrow)
    let mut sx = r.x;
    let mut sy = r.y + 2;
    for (t, on) in strip(data, st.board) {
        let w = str_w(&t) + 3;
        if sx + w > r.right() && sx > r.x {
            sx = r.x;
            sy += 1;
        }
        sx = put(buf, sx, sy, &t, if on { pen.fg(BLUE).add_modifier(Modifier::BOLD) } else { pen.fg(MUTED) }, r.right()) + 3;
    }
    let Some(b) = data["boards"].as_array().and_then(|l| l.get(st.board)) else { return };
    let body = Rect::new(r.x, sy + 2, r.width, r.bottom().saturating_sub(sy + 2));
    let c = board_canvas(pen, body.width, b);
    let top = st.scroll.min(max_scroll(c.area.height, body.height));
    blit(&c, buf, body, top);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> Value {
        serde_json::from_str(include_str!("../../tests/fixtures/leaderboards.json")).unwrap()
    }

    #[test]
    fn state_cycles() {
        let mut st = BoardsState { data: Some(Ok(fixture())), ..Default::default() };
        assert_eq!((st.window(), st.side()), ("7d", "organic"));
        st.cycle_window();
        st.cycle_side();
        assert_eq!((st.window(), st.side()), ("all", "house"));
        st.next_board(-1);
        assert_eq!(st.board, 8);
        st.next_board(1);
        assert_eq!(st.board, 0);
        assert_eq!(codes(2, 2), ("all", "all"));
    }

    #[test]
    fn canvas_draws_rows_and_rule() {
        let pen = Pen { mode: super::super::theme::Mode::NoColor };
        let j = fixture();
        let b = &j["boards"][0];
        for w in [80u16, 140] {
            let c = board_canvas(&pen, w, b);
            let text: String = (0..c.area.height).map(|y| (0..c.area.width).map(|x| c[(x, y)].symbol().to_string()).collect::<String>() + "\n").collect();
            assert!(text.contains("@tide_ledger"), "{text}");
            assert!(text.contains("Rule:"));
            assert!(!text.contains('\u{2014}'));
        }
    }
}
