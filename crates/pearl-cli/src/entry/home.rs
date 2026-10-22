//! The Home view: the entry screen as it was (the robot, the block wordmark, the Ask
//! prompt, the hints) and, once there is a conversation, the transcript above the prompt.

use std::time::Duration;

use ratatui::buffer::Buffer;
use ratatui::layout::{Position, Rect};
use ratatui::style::{Modifier, Style};
use unicode_width::UnicodeWidthChar;

use super::draw::{fill, put, str_w, wrap, Pen};
use super::robot::{self, Robot};
use super::theme::{self, Mode, BG, BLUE, DIM, GREEN, MUTED, PANEL, RED, TEXT};
use super::{logo, App, Kind};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RobotSize {
    Full,
    Half,
    None,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LogoKind {
    /// "cadre" and "agents" side by side (54 cells).
    Wide,
    /// "cadre" over "agents" (29 cells, 8 rows).
    Stacked,
    /// plain words, for very narrow terminals.
    Text,
}

/// Where everything goes on the start screen.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Plan {
    pub robot: RobotSize,
    pub robot_y: u16,
    pub logo: LogoKind,
    pub logo_y: u16,
    pub box_x: u16,
    pub box_w: u16,
    pub box_y: u16,
    pub hints_y: u16,
    /// Bottom bar row (None when the terminal is too short for it).
    pub footer_y: Option<u16>,
}

pub const BOX_ROWS: u16 = 5;
const MAX_BOX_W: u16 = 75;

pub fn footer_rows(h: u16) -> u16 {
    if h >= 16 {
        3
    } else if h >= 10 {
        1
    } else {
        0
    }
}

pub fn footer_y(h: u16) -> Option<u16> {
    match footer_rows(h) {
        3 => Some(h - 2),
        1 => Some(h - 1),
        _ => None,
    }
}

pub fn box_geom(w: u16) -> (u16, u16) {
    let margin = if w < 44 { 1 } else { 2 };
    let bw = w.saturating_sub(2 * margin).min(MAX_BOX_W).max(1).min(w);
    (w.saturating_sub(bw) / 2, bw)
}

/// The start screen: robot (full 44 x 24, half 22 x 12, or none), wordmark, prompt box,
/// hints, bottom bar; centered vertically in what the bottom bar leaves.
pub fn plan_home(w: u16, h: u16) -> Plan {
    let foot = footer_rows(h);
    let avail = h.saturating_sub(foot);
    let logo_h = |k: LogoKind| -> u16 {
        match k {
            LogoKind::Wide => 4,
            LogoKind::Stacked => 8,
            LogoKind::Text => 1,
        }
    };
    let mut logo = if w >= 58 {
        LogoKind::Wide
    } else if w >= 31 {
        LogoKind::Stacked
    } else {
        LogoKind::Text
    };
    // a short terminal keeps the prompt: the block wordmark gives way to plain words
    if logo_h(logo) + 1 + BOX_ROWS + 1 > avail {
        logo = LogoKind::Text;
    }
    let logo_h = logo_h(logo);
    let rest = logo_h + 1 + BOX_ROWS + 1;
    let robot = if w >= 44 && 24 + 1 + rest <= avail {
        RobotSize::Full
    } else if w >= 24 && 12 + 1 + rest <= avail {
        RobotSize::Half
    } else {
        RobotSize::None
    };
    let rh = match robot {
        RobotSize::Full => 25,
        RobotSize::Half => 13,
        RobotSize::None => 0,
    };
    let content = rh + rest;
    let top = avail.saturating_sub(content) / 2;
    let (box_x, box_w) = box_geom(w);
    let logo_y = top + rh;
    let box_y = logo_y + logo_h + 1;
    Plan { robot, robot_y: top, logo, logo_y, box_x, box_w, box_y, hints_y: box_y + BOX_ROWS, footer_y: footer_y(h) }
}

/// The conversation view: transcript on top, the prompt box at the bottom.
pub fn plan_chat(w: u16, h: u16) -> (Rect, Plan) {
    let foot = footer_rows(h);
    let (box_x, box_w) = box_geom(w);
    let box_y = h.saturating_sub(foot + BOX_ROWS + 1);
    let plan = Plan { robot: RobotSize::None, robot_y: 0, logo: LogoKind::Text, logo_y: 0, box_x, box_w, box_y, hints_y: box_y + BOX_ROWS, footer_y: footer_y(h) };
    let top = if h >= 20 { 1 } else { 0 };
    (Rect::new(box_x, top, box_w, box_y.saturating_sub(top + 1)), plan)
}

pub fn draw_robot(buf: &mut Buffer, pen: &Pen, r: &Robot, elapsed: Duration, y: u16, w: u16) {
    let x0 = w.saturating_sub(r.cols as u16) / 2;
    for (i, line) in r.frame_at(elapsed).iter().enumerate() {
        let yy = y + i as u16;
        for (j, ch) in line.chars().enumerate() {
            if ch == ' ' {
                continue;
            }
            let st = pen.fg(theme::ROBOT[robot::shade(ch) as usize]);
            put(buf, x0 + j as u16, yy, &ch.to_string(), st, w);
        }
    }
}

fn draw_word(buf: &mut Buffer, pen: &Pen, rows: &[String], x: u16, y: u16, ink: u32, bold: bool, w: u16) {
    let shadow = theme::tint(BG, ink, 0.25);
    for (i, row) in rows.iter().enumerate() {
        for (j, m) in row.chars().enumerate() {
            let (ch, ink_fg, on_shadow) = logo::cell(m);
            if ch == ' ' && !on_shadow {
                continue;
            }
            let mut st = if on_shadow { pen.on(ink, shadow) } else { pen.fg(if ink_fg { ink } else { shadow }) };
            if pen.mode == Mode::NoColor && !ink_fg {
                st = Style::default().add_modifier(Modifier::DIM);
            }
            if bold {
                st = st.add_modifier(Modifier::BOLD);
            }
            put(buf, x + j as u16, y + i as u16, &ch.to_string(), st, w);
        }
    }
}

pub fn draw_logo(buf: &mut Buffer, pen: &Pen, kind: LogoKind, y: u16, w: u16) {
    match kind {
        LogoKind::Wide => {
            let x = w.saturating_sub(logo::width("cadre", "agents") as u16) / 2;
            draw_word(buf, pen, &logo::word("cadre"), x, y, MUTED, false, w);
            draw_word(buf, pen, &logo::word("agents"), x + 25, y, TEXT, true, w);
        }
        LogoKind::Stacked => {
            let l = logo::word("cadre");
            let x = w.saturating_sub(29) / 2;
            draw_word(buf, pen, &l, x + 2, y, MUTED, false, w);
            draw_word(buf, pen, &logo::word("agents"), x, y + 4, TEXT, true, w);
        }
        LogoKind::Text => {
            let x = w.saturating_sub(12) / 2;
            let x = put(buf, x, y, "cadre", pen.fg(MUTED), w);
            put(buf, x, y, "agents", pen.fg(TEXT).add_modifier(Modifier::BOLD), w);
        }
    }
}

/// A prompt box (blue bar, panel, input or placeholder, the mode line, the half-block
/// bottom edge) at (x, y), `bw` wide. Returns the cursor position.
#[allow(clippy::too_many_arguments)]
pub fn prompt_box(buf: &mut Buffer, pen: &Pen, x: u16, y: u16, bw: u16, input: &[char], cur: usize, placeholder: &str, mode: &[(String, u32)]) -> Option<Position> {
    let right = x + bw;
    let bar = pen.fg(BLUE);
    for i in 0..4 {
        put(buf, x, y + i, "┃", bar, right);
    }
    put(buf, x, y + 4, "╹", bar, right);
    fill(buf, Rect::new(x + 1, y, bw.saturating_sub(1), 4), pen.bg(PANEL));
    if pen.mode != Mode::NoColor {
        for xx in x + 1..right {
            put(buf, xx, y + 4, "▀", pen.fg(PANEL), right);
        }
    }
    let pad = if buf.area.width < 44 { 1 } else { 2 };
    let tx = x + 1 + pad;
    let inner = right.saturating_sub(tx + pad);
    let cursor = if input.is_empty() {
        put(buf, tx, y + 1, placeholder, pen.on(theme::SUBTLE, PANEL), tx + inner);
        Some(Position::new(tx, y + 1))
    } else {
        let widths: Vec<u16> = input.iter().map(|c| c.width().unwrap_or(0) as u16).collect();
        let cur = cur.min(input.len());
        let mut start = 0;
        while widths[start..cur].iter().sum::<u16>() >= inner && start < cur {
            start += 1;
        }
        let shown: String = input[start..].iter().collect();
        put(buf, tx, y + 1, &shown, pen.on(TEXT, PANEL), tx + inner);
        Some(Position::new(tx + widths[start..cur].iter().sum::<u16>(), y + 1))
    };
    let mut xx = tx;
    for (i, (s, c)) in mode.iter().enumerate() {
        if i > 0 {
            xx += if i == 1 { 2 } else { 1 };
        }
        xx = put(buf, xx, y + 3, s, pen.on(*c, PANEL), tx + inner);
    }
    cursor
}

/// The Ask prompt: the mode label (blue), the model or the agent (text), the network or
/// the kind (muted).
pub(crate) fn draw_box(buf: &mut Buffer, pen: &Pen, app: &App, p: &Plan) -> Option<Position> {
    let mut mode = vec![(app.label(), BLUE)];
    match app.agent() {
        None => {
            match (&app.model, &app.model_err) {
                (Some(m), _) => mode.push((m.id.clone(), TEXT)),
                (None, Some(_)) => mode.push(("no model (gateway unavailable)".to_string(), MUTED)),
                (None, None) => mode.push(("reading models...".to_string(), MUTED)),
            }
            mode.push((format!("Cadre {}", app.network), MUTED));
        }
        Some(a) => {
            mode.push((a.name.clone(), TEXT));
            mode.push(("agent".to_string(), MUTED));
        }
    }
    prompt_box(buf, pen, p.box_x, p.box_y, p.box_w, &app.input, app.cur, &app.placeholder(), &mode)
}

pub(crate) fn draw_hints(buf: &mut Buffer, pen: &Pen, app: &App, p: &Plan) {
    let right = p.box_x + p.box_w;
    let y = p.hints_y;
    let back = format!(" {}", app.last_view.name());
    let parts: Vec<(&str, u32)> = if app.quote.is_some() {
        vec![("enter", TEXT), (" send DM", MUTED), ("  ", MUTED), ("esc", TEXT), (" cancel", MUTED)]
    } else if p.box_w >= 60 {
        vec![("tab", TEXT), (" switch agent", MUTED), ("  ", MUTED), ("esc", TEXT), (back.as_str(), MUTED), ("  ", MUTED), ("ctrl+p", TEXT), (" commands", MUTED)]
    } else {
        vec![("tab", TEXT), (" switch agent", MUTED), ("  ", MUTED), ("ctrl+p", TEXT), (" commands", MUTED)]
    };
    let w: u16 = parts.iter().map(|(s, _)| str_w(s)).sum();
    let mut x = right.saturating_sub(w);
    for (s, c) in &parts {
        x = put(buf, x, y, s, pen.fg(*c), right);
    }
    let i = app.start.elapsed().as_millis();
    let status = if app.busy.is_some() {
        Some(format!("{} answering", super::draw::spinner(i)))
    } else if app.quoting {
        Some(format!("{} pricing the DM", super::draw::spinner(i)))
    } else {
        None
    };
    if let Some(s) = status {
        put(buf, p.box_x + 1, y, &s, pen.fg(BLUE), right.saturating_sub(w + 1));
    }
}

/// Transcript lines: (text, fg, bar color for a "you" line).
pub(crate) fn transcript_lines(app: &App, w: u16) -> Vec<(String, u32, Option<u32>)> {
    let tw = w.saturating_sub(3) as usize;
    let mut out = vec![];
    for e in &app.transcript {
        if !out.is_empty() {
            out.push((String::new(), TEXT, None));
        }
        match e.kind {
            Kind::You => {
                out.push((String::new(), TEXT, Some(BLUE)));
                for l in wrap(&e.text, tw) {
                    out.push((l, TEXT, Some(BLUE)));
                }
                out.push((e.label.clone(), BLUE, Some(BLUE)));
                out.push((String::new(), TEXT, Some(BLUE)));
            }
            Kind::Answer => {
                let t = if e.text.is_empty() { "...".to_string() } else { e.text.trim_start().to_string() };
                for l in wrap(&t, tw) {
                    out.push((l, TEXT, None));
                }
                if !e.foot.is_empty() {
                    for l in wrap(&e.foot, tw) {
                        out.push((l, DIM, None));
                    }
                }
            }
            Kind::Note => {
                for l in wrap(&e.text, tw) {
                    out.push((l, MUTED, None));
                }
            }
            Kind::Error => {
                for l in wrap(&e.text, tw) {
                    out.push((l, RED, None));
                }
            }
            Kind::Card => {
                for (s, tone) in &e.lines {
                    let c = match tone {
                        0 => TEXT,
                        1 => MUTED,
                        _ => DIM,
                    };
                    for l in wrap(s, tw) {
                        out.push((l, c, None));
                    }
                }
            }
        }
    }
    if out.last().is_some_and(|l| l.0.starts_with("sent:") || l.0.contains(": sent:")) {
        if let Some(l) = out.last_mut() {
            l.1 = GREEN;
        }
    }
    out
}

pub(crate) fn draw_transcript(buf: &mut Buffer, pen: &Pen, app: &App, area: Rect) {
    let lines = transcript_lines(app, area.width);
    let h = area.height as usize;
    let max_scroll = lines.len().saturating_sub(h);
    let scroll = app.scroll.min(max_scroll);
    let end = lines.len() - scroll;
    let start = end.saturating_sub(h);
    // short transcripts sit just above the prompt, as in a chat
    let top = area.y + h.saturating_sub(end - start) as u16;
    let right = area.right();
    for (i, (s, c, bar)) in lines[start..end].iter().enumerate() {
        let y = top + i as u16;
        match bar {
            Some(b) => {
                put(buf, area.x, y, "┃", pen.fg(*b), right);
                fill(buf, Rect::new(area.x + 1, y, area.width.saturating_sub(1), 1), pen.bg(PANEL));
                put(buf, area.x + 3, y, s, pen.on(*c, PANEL), right);
            }
            None => {
                put(buf, area.x + 1, y, s, pen.fg(*c), right);
            }
        }
    }
    if scroll > 0 {
        let s = format!("{scroll} lines below");
        super::draw::put_right(buf, right, area.bottom().saturating_sub(1), &s, pen.fg(DIM), area.x);
    }
}

/// The Home view. Returns the cursor position.
pub(crate) fn draw(buf: &mut Buffer, pen: &Pen, app: &App, w: u16, h: u16) -> Option<Position> {
    let mut cursor = None;
    if app.transcript.is_empty() {
        let p = plan_home(w, h);
        match p.robot {
            RobotSize::Full => draw_robot(buf, pen, &app.robot, app.start.elapsed(), p.robot_y, w),
            RobotSize::Half => draw_robot(buf, pen, &app.robot_half, app.start.elapsed(), p.robot_y, w),
            RobotSize::None => {}
        }
        draw_logo(buf, pen, p.logo, p.logo_y, w);
        if p.box_y + BOX_ROWS <= h {
            cursor = draw_box(buf, pen, app, &p);
            draw_hints(buf, pen, app, &p);
        }
    } else {
        let (t, p) = plan_chat(w, h);
        // the mascot stays in the free space above a short conversation (owner 2026-10-21:
        // "add the mascot up there"): full size when it fits, else half, else none
        let used = transcript_lines(app, t.width).len().min(t.height as usize) as u16;
        let free = t.height.saturating_sub(used);
        let robot = if w >= 44 && free >= 25 + 2 { Some((&app.robot, 25u16)) } else if w >= 24 && free >= 13 + 2 { Some((&app.robot_half, 13u16)) } else { None };
        if let Some((r, rh)) = robot {
            let y = t.y + free.saturating_sub(rh + 1);
            draw_robot(buf, pen, r, app.start.elapsed(), y, w);
        }
        draw_transcript(buf, pen, app, t);
        cursor = draw_box(buf, pen, app, &p);
        draw_hints(buf, pen, app, &p);
    }
    cursor
}
