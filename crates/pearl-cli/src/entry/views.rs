//! The other views of the Cadre app, all in the entry style and all drawn from what the
//! shared poller (or a command run here) delivered: Agents, Inbox, Activity, Status and
//! Lookup. Scrollable content is drawn on a canvas (a buffer as tall as the content) and
//! the visible window is copied onto the screen, so scrolling never re-reads anything.

use std::collections::{HashMap, HashSet};

use ratatui::buffer::Buffer;
use ratatui::layout::{Position, Rect};
use ratatui::style::Modifier;
use serde_json::Value;

use super::draw::{self, card, clip, put, put_right, selected_row, str_w, wrap, Pen};
use super::net::AgentEntry;
use super::theme::{BLUE, DIM, GREEN, MUTED, PANEL, RED, TEXT, YELLOW};
use crate::brand::TICKER;
use crate::commands::Out;
use crate::fmt::{self, short};
use crate::health::{Level, Report, GROUPS};
use crate::snapshot::{s, u, Snapshot};
use crate::term::{Block, Row, Tone};

// ---------------------------------------------------------------------------------------
// canvas

/// A buffer `w` wide and `h` tall to draw scrollable content on.
pub fn canvas(w: u16, h: u16) -> Buffer {
    Buffer::empty(Rect::new(0, 0, w.max(1), h.max(1)))
}

/// Copy rows `top..top + area.height` of `src` onto `dst` at `area`.
pub fn blit(src: &Buffer, dst: &mut Buffer, area: Rect, top: u16) {
    let area = area.intersection(dst.area);
    for y in 0..area.height {
        let sy = top + y;
        if sy >= src.area.height {
            break;
        }
        for x in 0..area.width.min(src.area.width) {
            let c = src[(x, sy)].clone();
            if let Some(d) = dst.cell_mut(Position::new(area.x + x, area.y + y)) {
                *d = c;
            }
        }
    }
}

/// Lines from the top a scroll offset may reach for content `h` tall in `vis` rows.
pub fn max_scroll(h: u16, vis: u16) -> u16 {
    h.saturating_sub(vis)
}

/// "12:03:41" (UTC) of a time in ms.
pub fn hms(ms: u64) -> String {
    let s = ms / 1000 % 86400;
    format!("{:02}:{:02}:{:02}", s / 3600, s / 60 % 60, s % 60)
}

// ---------------------------------------------------------------------------------------
// Activity

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum EvKind {
    Block,
    Tx,
    Post,
    Trade,
}

impl EvKind {
    pub fn word(self) -> &'static str {
        match self {
            EvKind::Block => "block",
            EvKind::Tx => "tx",
            EvKind::Post => "post",
            EvKind::Trade => "trade",
        }
    }
    fn color(self) -> u32 {
        match self {
            EvKind::Block => BLUE,
            EvKind::Tx => MUTED,
            EvKind::Post => GREEN,
            EvKind::Trade => YELLOW,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Event {
    pub key: String,
    pub kind: EvKind,
    /// ms since the epoch (0 when unknown).
    pub time_ms: u64,
    /// Order inside one time (a block above its txs).
    pub seq: u32,
    pub head: String,
    pub text: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Filter {
    All,
    Blocks,
    Txs,
    Posts,
    Trades,
}

impl Filter {
    pub const ALL: [Filter; 5] = [Filter::All, Filter::Blocks, Filter::Txs, Filter::Posts, Filter::Trades];
    pub fn name(self) -> &'static str {
        match self {
            Filter::All => "all",
            Filter::Blocks => "blocks",
            Filter::Txs => "txs",
            Filter::Posts => "posts",
            Filter::Trades => "trades",
        }
    }
    pub fn key(self) -> char {
        match self {
            Filter::All => 'a',
            Filter::Blocks => 'b',
            Filter::Txs => 't',
            Filter::Posts => 'p',
            Filter::Trades => 'x',
        }
    }
    pub fn admits(self, k: EvKind) -> bool {
        match self {
            Filter::All => true,
            Filter::Blocks => k == EvKind::Block,
            Filter::Txs => k == EvKind::Tx,
            Filter::Posts => k == EvKind::Post,
            Filter::Trades => k == EvKind::Trade,
        }
    }
}

/// Everything seen since the app started, newest first, plus where the reader is.
#[derive(Debug)]
pub struct Activity {
    pub events: Vec<Event>,
    seen: HashSet<String>,
    pub filter: Filter,
    /// Rows scrolled down from the newest (0 = following the newest).
    pub scroll: usize,
}

impl Default for Activity {
    fn default() -> Self {
        Activity { events: vec![], seen: HashSet::new(), filter: Filter::All, scroll: 0 }
    }
}

pub const ACTIVITY_CAP: usize = 600;

/// Words for a trade from cadre-api `/v1/trading/trades` (amounts as the API gives them).
pub fn trade_words(t: &Value, handles: &HashMap<String, String>) -> String {
    let who = handles.get(s(t, "agent")).map(|h| format!("@{h}")).unwrap_or_else(|| fmt::short_addr(s(t, "agent")));
    let side = s(t, "side");
    let (ain, aout) = (u(t, "amount_in") as u128, u(t, "amount_out") as u128);
    let what = if s(t, "unit") == "lamports" {
        if side == "buy" {
            format!("bought {} {TICKER} for {} SOL", fmt::spl(aout), fmt::sol(ain))
        } else {
            format!("sold {} {TICKER} for {} SOL", fmt::spl(ain), fmt::sol(aout))
        }
    } else {
        format!("{side}: {} in, {} out ({})", s(t, "amount_in"), s(t, "amount_out"), s(t, "unit"))
    };
    let house = if t["house"] == true { ", house" } else { "" };
    format!("{who} {what} on {}{house}", s(t, "venue"))
}

impl Activity {
    /// Add what is new; returns how many visible rows were added above the reader.
    pub fn ingest(&mut self, mut new: Vec<Event>) -> usize {
        new.retain(|e| !self.seen.contains(&e.key));
        if new.is_empty() {
            return 0;
        }
        let shown = new.iter().filter(|e| self.filter.admits(e.kind)).count();
        for e in &new {
            self.seen.insert(e.key.clone());
        }
        self.events.extend(new);
        self.events.sort_by(|a, b| (b.time_ms, b.seq).cmp(&(a.time_ms, a.seq)));
        if self.events.len() > ACTIVITY_CAP {
            for e in self.events.drain(ACTIVITY_CAP..) {
                self.seen.remove(&e.key);
            }
        }
        // the reader keeps looking at the same rows when new ones arrive on top
        if self.scroll > 0 {
            self.scroll += shown;
        }
        shown
    }

    pub fn visible(&self) -> Vec<&Event> {
        self.events.iter().filter(|e| self.filter.admits(e.kind)).collect()
    }
}

/// Blocks and their txs of a snapshot as events.
pub fn chain_events(sn: &Snapshot, handles: &HashMap<String, String>) -> Vec<Event> {
    let mut out = vec![];
    for b in &sn.blocks {
        let hf = &b["header_fields"];
        let h = u(hf, "height");
        let t = u(hf, "timestamp_ms");
        let job = s(hf, "job_id");
        let fallback = job.chars().all(|c| c == '0');
        let txs = b["txs"].as_array().cloned().unwrap_or_default();
        let miner = s(hf, "miner");
        let who = handles.get(miner).map(|h| format!("@{h}")).unwrap_or_else(|| fmt::short_addr(miner));
        out.push(Event {
            key: format!("b{}", s(b, "hash")),
            kind: EvKind::Block,
            time_ms: t,
            seq: 1000,
            head: format!("block {h}"),
            text: format!("{} · {} txs · miner {who} · reward {} {TICKER}", if fallback { "fallback".to_string() } else { format!("job {}", short(job)) }, txs.len(), fmt::tokens(u(b, "reward") as u128)),
        });
        // a run of txs that read the same (32 PostSegments of one ticket) is one line
        let mut run: Option<(Event, usize)> = None;
        for (i, x) in txs.iter().enumerate() {
            let tx = crate::decode::chain_tx_b64(s(x, "tx"));
            let mut w = crate::decode::chain_words(tx.as_ref(), s(x, "kind"), s(x, "from"), &sn.params);
            let from = s(x, "from");
            if let Some(hh) = handles.get(from) {
                w.headline = w.headline.replacen(&fmt::short_addr(from), &format!("@{hh}"), 1).replacen(&short(from), &format!("@{hh}"), 1);
            }
            let e = Event { key: format!("t{}", s(x, "tx_hash")), kind: EvKind::Tx, time_ms: t, seq: 999 - i as u32, head: w.kind.clone(), text: w.headline };
            match &mut run {
                Some((r, n)) if r.head == e.head && r.text == e.text => *n += 1,
                _ => {
                    if let Some(done) = run.take() {
                        out.push(flush(done));
                    }
                    run = Some((e, 1));
                }
            }
        }
        if let Some(done) = run.take() {
            out.push(flush(done));
        }
    }
    out
}

fn flush((mut e, n): (Event, usize)) -> Event {
    if n > 1 {
        e.text = format!("{}  ({n} txs like this)", e.text);
    }
    e
}

pub fn post_events(posts: &[Value]) -> Vec<Event> {
    posts
        .iter()
        .map(|p| Event {
            key: format!("p{}", s(p, "post_id")),
            kind: EvKind::Post,
            time_ms: u(p, "timestamp_ms"),
            seq: 500,
            head: format!("@{}", p["author"]["handle"].as_str().unwrap_or("?")),
            text: p["body"].as_str().unwrap_or("").replace('\n', " "),
        })
        .collect()
}

/// Trades; Solana ones carry a time in ms, Cadre ones the block height (its time comes
/// from the blocks already seen, else the trade sorts last).
pub fn trade_events(trades: &[Value], handles: &HashMap<String, String>, block_ms: &HashMap<u64, u64>) -> Vec<Event> {
    trades
        .iter()
        .map(|t| {
            let at = u(t, "at");
            let time = if s(t, "chain") == "solana" { at } else { block_ms.get(&at).copied().unwrap_or(0) };
            Event { key: format!("x{}", s(t, "ref")), kind: EvKind::Trade, time_ms: time, seq: 400, head: s(t, "venue").to_string(), text: trade_words(t, handles) }
        })
        .collect()
}

/// The Activity body: a filter bar, then one line per event (wrapped posts take two).
pub fn draw_activity(buf: &mut Buffer, pen: &Pen, r: Rect, a: &Activity, errors: &[String], ms: u128, loading: bool) {
    let mut x = r.x;
    for f in Filter::ALL {
        let on = f == a.filter;
        x = put(buf, x, r.y, &f.key().to_string(), pen.fg(if on { BLUE } else { DIM }), r.right());
        x = put(buf, x + 1, r.y, f.name(), if on { pen.fg(BLUE).add_modifier(Modifier::BOLD) } else { pen.fg(MUTED) }, r.right());
        x += 3;
    }
    let ev = a.visible();
    let status = if a.scroll > 0 { format!("{} newer above · g follows the newest · times UTC", a.scroll) } else if loading { format!("{} following the newest · times UTC", draw::spinner(ms)) } else { "following the newest · times UTC".into() };
    put_right(buf, r.right(), r.y, &status, pen.fg(DIM), x + 2);
    let body = Rect::new(r.x, r.y + 2, r.width, r.height.saturating_sub(2));
    if ev.is_empty() {
        let msg = if !errors.is_empty() { errors.join("; ") } else if loading { "reading blocks, posts and trades".into() } else { format!("nothing yet under {}", a.filter.name()) };
        put(buf, body.x + 2, body.y, &clip(&msg, body.width as usize), pen.fg(MUTED), body.right());
        return;
    }
    let tw = body.width.saturating_sub(2 + 9 + 7 + 18) as usize;
    let mut y = body.y;
    for e in ev.iter().skip(a.scroll) {
        if y >= body.bottom() {
            break;
        }
        let x0 = body.x + if e.kind == EvKind::Tx { 4 } else { 2 };
        put(buf, body.x + 2, y, &if e.time_ms > 0 { hms(e.time_ms) } else { "        ".into() }, pen.fg(DIM), body.right());
        let kx = body.x + 12;
        put(buf, kx, y, e.kind.word(), pen.fg(e.kind.color()), kx + 6);
        let hx = kx + 7 + (x0 - body.x - 2);
        let hw = if e.kind == EvKind::Block { 11 } else { 16 };
        let head_style = if e.kind == EvKind::Block { pen.fg(TEXT).add_modifier(Modifier::BOLD) } else { pen.fg(TEXT) };
        put(buf, hx, y, &clip(&e.head, hw as usize), head_style, hx + hw);
        let txs = hx + hw + 1;
        let tc = if e.kind == EvKind::Block || e.kind == EvKind::Tx { MUTED } else { TEXT };
        put(buf, txs, y, &clip(&e.text, body.right().saturating_sub(txs) as usize), pen.fg(tc), body.right());
        y += 1;
        let _ = tw;
    }
}

// ---------------------------------------------------------------------------------------
// Status

/// Status cards (one per check group) laid out in one or two columns on a canvas.
pub fn status_canvas(pen: &Pen, w: u16, r: &Report) -> Buffer {
    let cols = if w >= 150 { 2 } else { 1 };
    let gap = 2u16;
    let cw = (w - gap * (cols - 1)) / cols;
    let groups: Vec<(&str, Vec<&crate::health::Check>)> = GROUPS.iter().map(|g| (*g, r.checks.iter().filter(|c| c.group == *g).collect::<Vec<_>>())).filter(|(_, v)| !v.is_empty()).collect();
    let total: u16 = groups.iter().map(|(_, v)| v.len() as u16 + 6).sum();
    let mut c = canvas(w, total + 2);
    let mut ys = vec![0u16; cols as usize];
    for (g, checks) in &groups {
        let col = (0..cols as usize).min_by_key(|i| ys[*i]).unwrap();
        let x = col as u16 * (cw + gap);
        let h = checks.len() as u16 + 5;
        let inner = card(&mut c, pen, Rect::new(x, ys[col], cw, h));
        let worst = checks.iter().map(|c| c.level).filter(|l| *l != Level::Skip).max().unwrap_or(Level::Pass);
        draw::title(&mut c, pen, inner, g);
        put_right(&mut c, inner.right(), inner.y, &format!("{} checks", checks.len()), pen.card(MUTED), inner.x);
        let namew = checks.iter().map(|c| str_w(&c.name)).max().unwrap_or(4).min(inner.width / 3);
        for (i, ck) in checks.iter().enumerate() {
            let y = inner.y + 2 + i as u16;
            let (word, color) = level_word(ck.level);
            put(&mut c, inner.x, y, word, pen.card(color).add_modifier(if ck.level == Level::Fail { Modifier::BOLD } else { Modifier::empty() }), inner.x + 4);
            put(&mut c, inner.x + 6, y, &clip(&ck.name, namew as usize), pen.card(TEXT), inner.x + 6 + namew);
            let msw = ck.ms.map(|m| format!("{m} ms")).unwrap_or_default();
            let dx = inner.x + 6 + namew + 2;
            let room = inner.right().saturating_sub(dx + str_w(&msw) + 2);
            put(&mut c, dx, y, &clip(&ck.detail, room as usize), pen.card(if ck.level == Level::Skip { DIM } else { MUTED }), dx + room);
            put_right(&mut c, inner.right(), y, &msw, pen.card(DIM), dx);
        }
        let _ = worst;
        ys[col] += h + 1;
    }
    let used = ys.iter().max().copied().unwrap_or(0);
    let mut out = canvas(w, used.max(1));
    blit(&c, &mut out, Rect::new(0, 0, w, used.max(1)), 0);
    out
}

pub fn level_word(l: Level) -> (&'static str, u32) {
    match l {
        Level::Pass => ("pass", GREEN),
        Level::Warn => ("warn", YELLOW),
        Level::Fail => ("fail", RED),
        Level::Skip => ("skip", DIM),
    }
}

/// The summary line over the status cards: counts by level and the run time.
pub fn status_summary(r: &Report) -> Vec<(String, u32)> {
    let c = |l| r.checks.iter().filter(|x| x.level == l).count();
    vec![
        (format!("{} pass", c(Level::Pass)), GREEN),
        ("  ".into(), MUTED),
        (format!("{} warn", c(Level::Warn)), if c(Level::Warn) > 0 { YELLOW } else { MUTED }),
        ("  ".into(), MUTED),
        (format!("{} fail", c(Level::Fail)), if c(Level::Fail) > 0 { RED } else { MUTED }),
        ("  ".into(), MUTED),
        (format!("{} skip", c(Level::Skip)), MUTED),
        (format!("  ·  checked in {:.1} s", r.elapsed_ms as f64 / 1000.0), DIM),
    ]
}

// ---------------------------------------------------------------------------------------
// command output (Lookup view): a CLI `View` drawn as cards

/// Height of a command's output when drawn `w` wide.
pub fn out_height(o: &Result<Out, String>, w: u16) -> u16 {
    match o {
        Err(e) => wrap(e, w.saturating_sub(2) as usize).len() as u16 + 1,
        Ok(o) => match &o.view {
            None => wrap(&o.text, w.saturating_sub(2) as usize).len() as u16 + 1,
            Some(v) => v.0.iter().map(|b| block_height(b, w) + 1).sum(),
        },
    }
}

/// Inner width of a card `w` wide (card(): bar, padding on both sides).
fn card_inner(w: u16) -> u16 {
    let pad = if w >= 30 { 2 } else { 1 };
    w.saturating_sub(1 + 2 * pad)
}

fn label_w(p: &crate::term::Panel, inner: u16) -> u16 {
    p.rows.iter().filter_map(|r| if let Row::Kv(k, _, _) = r { Some(str_w(k)) } else { None }).max().unwrap_or(0).min(inner / 2)
}

fn block_height(b: &Block, w: u16) -> u16 {
    let inner = card_inner(w);
    match b {
        Block::Panel(p) => {
            let lw = label_w(p, inner);
            let rows: u16 = p
                .rows
                .iter()
                .map(|r| match r {
                    Row::Kv(_, v, _) => wrap(v, inner.saturating_sub(lw + 2) as usize).len() as u16,
                    Row::Text(t, _) => wrap(t, inner as usize).len() as u16,
                    _ => 1,
                })
                .sum();
            // top padding, title + blank, rows, bottom padding + edge
            1 + if p.chip.is_some() { 2 } else { 0 } + rows + 2
        }
        Block::Table(t) => 1 + if t.chip.is_some() { 2 } else { 0 } + 1 + t.rows.len().max(1) as u16 + 2,
        Block::Hint(h) => h.lines().map(|l| wrap(l, w as usize).len() as u16).sum(),
        Block::Step(_, t) => wrap(t, w.saturating_sub(2) as usize).len() as u16,
    }
}

/// Draw a command's output at (x, y) on a canvas `w` wide; returns the next y.
pub fn draw_out(c: &mut Buffer, pen: &Pen, x: u16, y: u16, w: u16, o: &Result<Out, String>) -> u16 {
    match o {
        Err(e) => {
            let mut y = y;
            for l in wrap(e, w.saturating_sub(2) as usize) {
                put(c, x + 1, y, &l, pen.fg(RED), x + w);
                y += 1;
            }
            y + 1
        }
        Ok(o) => match &o.view {
            None => {
                let mut y = y;
                for l in wrap(&o.text, w.saturating_sub(2) as usize) {
                    put(c, x + 1, y, &l, pen.fg(TEXT), x + w);
                    y += 1;
                }
                y + 1
            }
            Some(v) => {
                let mut y = y;
                for b in &v.0 {
                    let h = block_height(b, w);
                    draw_block(c, pen, Rect::new(x, y, w, h), b);
                    y += h + 1;
                }
                y
            }
        },
    }
}

fn tone_c(t: Tone) -> u32 {
    draw::tone(t)
}

fn draw_block(c: &mut Buffer, pen: &Pen, r: Rect, b: &Block) {
    match b {
        Block::Panel(p) => {
            let inner = card(c, pen, r);
            let mut y = inner.y;
            if let Some(ch) = &p.chip {
                draw::title(c, pen, inner, ch);
                y += 2;
            }
            let lw = label_w(p, inner.width);
            for row in &p.rows {
                if y >= inner.bottom() {
                    break;
                }
                match row {
                    Row::Kv(k, v, t) => {
                        put(c, inner.x, y, &clip(k, lw as usize), pen.card(MUTED), inner.x + lw);
                        let vw = inner.width.saturating_sub(lw + 2) as usize;
                        let lines = wrap(v, vw);
                        for (i, l) in lines.iter().enumerate() {
                            if y + i as u16 >= inner.bottom() {
                                break;
                            }
                            put(c, inner.x + lw + 2, y + i as u16, l, pen.card(tone_c(*t)), inner.right());
                        }
                        // a value wrapped onto 3+ lines used to land its third line on the next row
                        y += lines.len().max(1) as u16;
                    }
                    Row::Section(t) => {
                        put(c, inner.x, y, t, pen.card(BLUE), inner.right());
                        y += 1;
                    }
                    Row::Blank => y += 1,
                    Row::Text(t, tone) => {
                        for l in wrap(t, inner.width as usize) {
                            put(c, inner.x, y, &l, pen.card(tone_c(*tone)), inner.right());
                            y += 1;
                        }
                    }
                }
            }
        }
        Block::Table(t) => {
            let inner = card(c, pen, r);
            let mut y = inner.y;
            if let Some(ch) = &t.chip {
                draw::title(c, pen, inner, ch);
                y += 2;
            }
            // column widths from the content, the widest shrunk to fit
            let n = t.head.len();
            let mut ws: Vec<u16> = (0..n).map(|i| t.rows.iter().map(|r| r.get(i).map(|c| str_w(c.0.trim_end())).unwrap_or(0)).max().unwrap_or(0).max(str_w(&t.head[i]))).collect();
            let room = inner.width.saturating_sub(2 * n.saturating_sub(1) as u16);
            while ws.iter().sum::<u16>() > room {
                let i = (0..n).max_by_key(|i| ws[*i]).unwrap();
                if ws[i] <= 4 {
                    break;
                }
                ws[i] -= 1;
            }
            let mut x = inner.x;
            for (i, h) in t.head.iter().enumerate() {
                if t.right.get(i) == Some(&true) {
                    put_right(c, x + ws[i], y, h, pen.card(MUTED), x);
                } else {
                    put(c, x, y, h, pen.card(MUTED), x + ws[i]);
                }
                x += ws[i] + 2;
            }
            y += 1;
            if t.rows.is_empty() {
                put(c, inner.x, y, &t.empty, pen.card(MUTED), inner.right());
            }
            for row in &t.rows {
                let mut x = inner.x;
                for (i, (cell, tone)) in row.iter().enumerate() {
                    let w = ws.get(i).copied().unwrap_or(0);
                    let s = clip(cell.trim_end(), w as usize);
                    if t.right.get(i) == Some(&true) {
                        put_right(c, x + w, y, &s, pen.card(tone_c(*tone)), x);
                    } else {
                        put(c, x, y, &s, pen.card(tone_c(*tone)), x + w);
                    }
                    x += w + 2;
                }
                y += 1;
            }
        }
        Block::Hint(h) => {
            let mut y = r.y;
            for line in h.lines() {
                for l in wrap(line, r.width as usize) {
                    put(c, r.x + 1, y, &l, pen.fg(MUTED), r.right());
                    y += 1;
                }
            }
        }
        Block::Step(ok, t) => {
            put(c, r.x + 1, r.y, if *ok { "✓" } else { "✗" }, pen.fg(if *ok { GREEN } else { RED }), r.right());
            for (i, l) in wrap(t, r.width.saturating_sub(4) as usize).iter().enumerate() {
                put(c, r.x + 3, r.y + i as u16, l, pen.fg(TEXT), r.right());
            }
        }
    }
}

/// One query in the Lookup view and its answer (None while it runs).
#[derive(Debug, Clone)]
pub struct Lookup {
    pub id: u64,
    pub query: String,
    pub out: Option<Result<Out, String>>,
}

/// All lookups as one canvas, oldest first, and the row where the newest one starts.
pub fn lookup_canvas(pen: &Pen, w: u16, list: &[Lookup], ms: u128) -> (Buffer, u16) {
    let h: u16 = list.iter().map(|l| 2 + l.out.as_ref().map(|o| out_height(o, w)).unwrap_or(2)).sum::<u16>() + 1;
    let mut c = canvas(w, h);
    let mut y = 0;
    let mut last = 0;
    for l in list {
        last = y;
        selected_row(&mut c, pen, 0, y, w);
        put(&mut c, 2, y, &l.query, pen.card(TEXT), w);
        y += 2;
        match &l.out {
            None => {
                put(&mut c, 1, y, &format!("{} running", draw::spinner(ms)), pen.fg(BLUE), w);
                y += 2;
            }
            Some(o) => y = draw_out(&mut c, pen, 0, y, w, o),
        }
    }
    (c, last)
}

// ---------------------------------------------------------------------------------------
// Agents and Inbox

#[derive(Debug, Default)]
pub struct AgentsState {
    /// Selected agent by account.
    pub sel: Option<String>,
    pub scroll: usize,
    /// Profile cards read so far, by account.
    pub cards: HashMap<String, Result<Vec<(String, u8)>, String>>,
    /// The right pane's scroll.
    pub card_scroll: u16,
}

impl AgentsState {
    pub fn index(&self, list: &[AgentEntry]) -> usize {
        self.sel.as_ref().and_then(|a| list.iter().position(|x| &x.account == a)).unwrap_or(0)
    }
}

/// Is DM sending live, and if not, why (from `/v1/dm/params` and the chain tip).
pub fn dm_state(params: Option<&Result<Value, String>>, tip: Option<u64>, network: &str) -> (bool, String) {
    match params {
        None => (false, "reading /v1/dm/params".into()),
        Some(Err(e)) => (false, format!("DMs unavailable: {e}")),
        Some(Ok(p)) if p["active"] == true => (true, "DMs are live".into()),
        Some(Ok(p)) => {
            let at = p["activation_height"].as_u64();
            match (at, tip) {
                (Some(a), Some(t)) => (false, format!("DMs are not live on {network} yet: they activate at block {a} (tip {t}) once the nodes run the DM code")),
                (Some(a), None) => (false, format!("DMs are not live on {network} yet: they activate at block {a}")),
                _ => (false, format!("DMs are not live on {network} yet")),
            }
        }
    }
}

pub fn draw_agents(buf: &mut Buffer, pen: &Pen, r: Rect, list: &[AgentEntry], err: Option<&String>, st: &AgentsState, dm: &(bool, String), ms: u128) {
    if list.is_empty() {
        let msg = match err {
            Some(e) => format!("agents unavailable: {e}"),
            None => format!("{} reading /v1/profiles", draw::spinner(ms)),
        };
        put(buf, r.x + 2, r.y, &msg, pen.fg(MUTED), r.right());
        return;
    }
    let lw = (r.width * 40 / 100).clamp(30, 56).min(r.width);
    let left = Rect::new(r.x, r.y, lw, r.height);
    let house = list.iter().filter(|a| a.house).count();
    put(buf, left.x + 2, left.y, &format!("{} agents", list.len()), pen.fg(TEXT).add_modifier(Modifier::BOLD), left.right());
    put_right(buf, left.right(), left.y, &format!("{house} house · {} user", list.len() - house), pen.fg(MUTED), left.x);
    let vis = left.height.saturating_sub(2) as usize;
    let sel = st.index(list);
    let first = st.scroll.min(list.len().saturating_sub(1));
    let first = if sel < first { sel } else if sel >= first + vis { sel + 1 - vis } else { first };
    for (i, a) in list.iter().enumerate().skip(first).take(vis) {
        let y = left.y + 2 + (i - first) as u16;
        let on = i == sel;
        if on {
            selected_row(buf, pen, left.x, y, left.right());
        }
        let st_ = |c: u32| if on { pen.on(c, PANEL) } else { pen.fg(c) };
        let x = put(buf, left.x + 2, y, &format!("@{}", a.handle), st_(TEXT).add_modifier(if on { Modifier::BOLD } else { Modifier::empty() }), left.right());
        let kind = if a.house { "house" } else { "user" };
        let kx = left.right().saturating_sub(6);
        put(buf, x + 2, y, &clip(&a.name, kx.saturating_sub(x + 4) as usize), st_(MUTED), kx);
        put(buf, kx + 1, y, kind, st_(if a.house { BLUE } else { DIM }), left.right());
    }
    // right: the selected agent's card
    let rx = left.right() + 2;
    if rx + 20 > r.right() {
        return;
    }
    let right = Rect::new(rx, r.y, r.right() - rx, r.height);
    let a = &list[sel];
    let dm_line = if dm.0 { format!("m  message @{} on Home (the cost is shown before it is sent)", a.handle) } else { dm.1.clone() };
    let lines: Vec<(String, u8)> = match st.cards.get(&a.account) {
        None => vec![(format!("{} reading @{}", draw::spinner(ms), a.handle), 2)],
        Some(Err(e)) => vec![(format!("profile unavailable: {e}"), 2)],
        Some(Ok(l)) => l.clone(),
    };
    let tw = right.width.saturating_sub(5) as usize;
    let mut wrapped: Vec<(String, u32)> = vec![];
    for (s, tone) in &lines {
        let c = match tone {
            0 => TEXT,
            1 => MUTED,
            _ => DIM,
        };
        for l in wrap(s, tw) {
            wrapped.push((l, c));
        }
    }
    let h = (wrapped.len() as u16 + 4).min(right.height.saturating_sub(3)).max(4);
    let inner = card(buf, pen, Rect::new(right.x, right.y, right.width, h));
    let sc = (st.card_scroll as usize).min(wrapped.len().saturating_sub(inner.height as usize));
    for (i, (l, c)) in wrapped.iter().skip(sc).enumerate() {
        let y = inner.y + i as u16;
        if y >= inner.bottom() {
            break;
        }
        let stl = if i + sc == 0 { pen.card(*c).add_modifier(Modifier::BOLD) } else { pen.card(*c) };
        put(buf, inner.x, y, l, stl, inner.right());
    }
    for (i, l) in wrap(&dm_line, right.width.saturating_sub(2) as usize).iter().enumerate() {
        put(buf, right.x + 1, right.y + h + 1 + i as u16, l, pen.fg(if dm.0 { TEXT } else { MUTED }), right.right());
    }
}

#[derive(Debug, Default)]
pub struct InboxState {
    /// Selected thread by peer account.
    pub sel: Option<String>,
    /// Messages of opened threads, by peer.
    pub threads: HashMap<String, Result<Value, String>>,
}

pub fn draw_inbox(buf: &mut Buffer, pen: &Pen, r: Rect, me: Option<&str>, dm: &(bool, String), inbox: Option<&Result<Value, String>>, st: &InboxState, ms: u128) {
    let x = r.x + 2;
    let Some(me) = me else {
        put(buf, x, r.y, super::dash::NO_WALLET, pen.fg(MUTED), r.right());
        put(buf, x, r.y + 1, "or cadre --address ADDR to read an inbox (sealed bodies stay sealed)", pen.fg(DIM), r.right());
        return;
    };
    put(buf, x, r.y, &format!("Inbox of {}", fmt::short_addr(me)), pen.fg(TEXT).add_modifier(Modifier::BOLD), r.right());
    if !dm.0 {
        for (i, l) in wrap(&dm.1, r.width.saturating_sub(4) as usize).iter().enumerate() {
            put(buf, x, r.y + 2 + i as u16, l, pen.fg(MUTED), r.right());
        }
        return;
    }
    let v = match inbox {
        None => {
            put(buf, x, r.y + 2, &format!("{} reading /v1/inbox", draw::spinner(ms)), pen.fg(MUTED), r.right());
            return;
        }
        Some(Err(e)) => {
            put(buf, x, r.y + 2, &clip(&format!("inbox unavailable: {e}"), r.width as usize - 4), pen.fg(MUTED), r.right());
            return;
        }
        Some(Ok(v)) => v,
    };
    let c = &v["counts"];
    put_right(buf, r.right(), r.y, &format!("{} primary · {} requests · {} blocked", u(c, "primary"), u(c, "requests"), u(c, "blocked")), pen.fg(MUTED), x);
    let threads = v["threads"].as_array().cloned().unwrap_or_default();
    if threads.is_empty() {
        put(buf, x, r.y + 2, "no conversations yet", pen.fg(MUTED), r.right());
        return;
    }
    let lw = (r.width * 45 / 100).clamp(30, 64);
    let sel = st.sel.as_ref().and_then(|p| threads.iter().position(|t| s(&t["peer"], "account") == p)).unwrap_or(0);
    for (i, t) in threads.iter().enumerate().take(r.height.saturating_sub(2) as usize) {
        let y = r.y + 2 + i as u16;
        let on = i == sel;
        if on {
            selected_row(buf, pen, r.x, y, r.x + lw);
        }
        let st_ = |c: u32| if on { pen.on(c, PANEL) } else { pen.fg(c) };
        let p = &t["peer"];
        let who = p["handle"].as_str().map(|h| format!("@{h}")).unwrap_or_else(|| fmt::short_addr(s(p, "account")));
        let nx = put(buf, x, y, &who, st_(TEXT), r.x + lw);
        let last = if t["sealed"] == true { "sealed message".to_string() } else { t["last"]["body"].as_str().unwrap_or("").replace('\n', " ") };
        let tail = format!("{} · {}", u(t, "count"), s(t, "box"));
        let tx = (r.x + lw).saturating_sub(str_w(&tail) + 1);
        put(buf, nx + 2, y, &clip(&last, tx.saturating_sub(nx + 3) as usize), st_(MUTED), tx);
        put(buf, tx, y, &tail, st_(DIM), r.x + lw);
    }
    // right: the open thread
    let rx = r.x + lw + 2;
    if rx + 20 > r.right() {
        return;
    }
    let peer = s(&threads[sel]["peer"], "account").to_string();
    let right = Rect::new(rx, r.y + 2, r.right() - rx, r.height.saturating_sub(2));
    let msgs = match st.threads.get(&peer) {
        None => {
            put(buf, right.x, right.y, "enter opens the conversation", pen.fg(MUTED), right.right());
            return;
        }
        Some(Err(e)) => {
            put(buf, right.x, right.y, &clip(&format!("thread unavailable: {e}"), right.width as usize), pen.fg(MUTED), right.right());
            return;
        }
        Some(Ok(v)) => v["messages"].as_array().cloned().unwrap_or_default(),
    };
    let mut y = right.y;
    for m in msgs.iter().rev().take(40) {
        if y + 2 >= right.bottom() {
            break;
        }
        let mine = s(&m["from"], "account") == me;
        let who = m["from"]["handle"].as_str().map(|h| format!("@{h}")).unwrap_or_else(|| fmt::short_addr(s(&m["from"], "account")));
        let body = if m["mode"] == "plain" { m["body"].as_str().unwrap_or("").to_string() } else { format!("{} message", s(m, "mode")) };
        put(buf, right.x, y, &who, pen.fg(if mine { BLUE } else { TEXT }), right.right());
        put_right(buf, right.right(), y, &format!("block {} · {}", u(m, "height"), s(m, "status")), pen.fg(DIM), right.x);
        y += 1;
        for l in wrap(&body, right.width.saturating_sub(2) as usize).iter().take(4) {
            put(buf, right.x, y, l, pen.fg(MUTED), right.right());
            y += 1;
        }
        y += 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn activity_keeps_the_reader_in_place() {
        let mut a = Activity::default();
        let ev = |k: &str, t: u64, kind| Event { key: k.into(), kind, time_ms: t, seq: 0, head: k.into(), text: String::new() };
        assert_eq!(a.ingest(vec![ev("1", 10, EvKind::Block), ev("2", 20, EvKind::Post)]), 2);
        assert_eq!(a.events[0].key, "2");
        // duplicates are ignored
        assert_eq!(a.ingest(vec![ev("2", 20, EvKind::Post)]), 0);
        a.scroll = 1;
        a.ingest(vec![ev("3", 30, EvKind::Trade)]);
        assert_eq!(a.scroll, 2, "still on the same row");
        a.filter = Filter::Posts;
        assert_eq!(a.visible().len(), 1);
        a.ingest(vec![ev("4", 40, EvKind::Block)]);
        assert_eq!(a.scroll, 2, "a hidden row does not move the reader");
    }

    #[test]
    fn events_from_blocks_posts_and_trades() {
        let sn = crate::snapshot::sample::snapshot();
        let mut handles = HashMap::new();
        handles.insert(crate::snapshot::sample::ME.to_string(), "demo".to_string());
        let ev = chain_events(&sn, &handles);
        assert_eq!(ev.len(), 8);
        assert_eq!(ev[0].head, "block 260");
        assert!(ev[0].text.contains("miner @demo"), "{}", ev[0].text);
        assert!(ev[2].text.starts_with("job 0000...abcd"), "{}", ev[2].text);
        let p = post_events(&[json!({"post_id": "ab", "author": {"handle": "tide"}, "body": "gm\nall", "timestamp_ms": 5})]);
        assert_eq!((p[0].head.as_str(), p[0].text.as_str(), p[0].time_ms), ("@tide", "gm all", 5));
        let t = json!({"chain": "solana", "venue": "pumpswap", "agent": "A", "side": "buy", "amount_in": "4000000", "amount_out": "150318604619", "unit": "lamports", "at": 7, "ref": "sol:x", "house": true});
        let mut h = HashMap::new();
        h.insert("A".to_string(), "tape_reader".to_string());
        let x = trade_events(&[t], &h, &HashMap::new());
        assert_eq!(x[0].text, "@tape_reader bought 150,318.6046 CADRE for 0.004 SOL on pumpswap, house");
        assert_eq!(x[0].time_ms, 7);
        assert_eq!(hms(3_723_000), "01:02:03");
        // a run of txs that read the same is one line with its count
        let mut sn2 = Snapshot::default();
        let tx = |h: &str, k: &str| json!({"tx": "", "kind": k, "from": "F", "tx_hash": h});
        sn2.blocks = vec![json!({"hash": "b", "header_fields": {"height": 9, "miner": "M", "job_id": "00", "timestamp_ms": 1}, "reward": "0",
                                 "txs": [tx("h1", "PostSegments"), tx("h2", "PostSegments"), tx("h3", "Transfer")]})];
        let ev = chain_events(&sn2, &HashMap::new());
        assert_eq!(ev.len(), 3, "{ev:?}");
        assert!(ev[1].text.ends_with("(2 txs like this)"), "{}", ev[1].text);
        assert!(!ev[2].text.contains("like this"));
    }

    #[test]
    fn dm_state_says_why() {
        let p = Ok(json!({"active": false, "activation_height": 4800}));
        let (live, why) = dm_state(Some(&p), Some(4768), "devnet");
        assert!(!live);
        assert!(why.contains("block 4800") && why.contains("tip 4768"), "{why}");
        assert!(dm_state(Some(&Ok(json!({"active": true}))), None, "devnet").0);
        assert!(dm_state(Some(&Err("HTTP 503".into())), None, "devnet").1.contains("HTTP 503"));
    }

    #[test]
    fn command_output_draws_as_cards() {
        use crate::term::{Panel, View};
        let pen = Pen { mode: super::super::theme::Mode::TrueColor };
        let v = View::panel(Panel::new("Block 12").kv("Miner", "abc").kv("Txs", "3")).hint("a hint");
        let o: Result<Out, String> = Ok(Out::new(json!({}), "t").view(v));
        let l = vec![Lookup { id: 1, query: "block 12".into(), out: Some(o) }];
        let (c, last) = lookup_canvas(&pen, 60, &l, 0);
        assert_eq!(last, 0);
        let text: Vec<String> = (0..c.area.height).map(|y| (0..60).map(|x| c[(x, y)].symbol().to_string()).collect()).collect();
        let all = text.join("\n");
        assert!(text[0].contains("block 12"));
        assert!(all.contains("Block 12") && all.contains("Miner") && all.contains("abc") && all.contains("a hint"), "{all}");
        assert_eq!(c[(0, 0)].symbol(), "┃");
    }

    /// A value that wraps onto three lines keeps the next row below it (it used to overwrite it).
    #[test]
    fn long_values_do_not_overwrite_the_next_row() {
        use crate::term::{Panel, View};
        let pen = Pen { mode: super::super::theme::Mode::NoColor };
        let long = format!("@scribe_wren (house) {}", "Cx774dEaA57Ee1975B6b007135Ea586A1290Ec9F7694F9738435a5c9CE69228205");
        let o: Result<Out, String> = Ok(Out::new(json!({}), "t").view(View::panel(Panel::new("Offer").kv("Provider", long).kv("Price", "4 CADRE"))));
        let w = 50;
        let h = out_height(&o, w);
        let mut c = canvas(w, h);
        draw_out(&mut c, &pen, 0, 0, w, &o);
        let rows: Vec<String> = (0..h).map(|y| (0..w).map(|x| c[(x, y)].symbol().to_string()).collect()).collect();
        let price = rows.iter().position(|r| r.contains("Price")).unwrap();
        assert!(rows[price].contains("4 CADRE") && !rows[price].contains("CE69"), "{rows:#?}");
        assert!(rows[..price].iter().any(|r| r.contains("228205")), "{rows:#?}");
    }
}
