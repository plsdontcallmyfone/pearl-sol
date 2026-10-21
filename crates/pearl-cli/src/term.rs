//! Inline terminal output for plain mode: step lines (`✓ Included in block 812`), a
//! spinner on the step in progress, a green progress bar with elapsed time for waits,
//! and gray-bordered info boxes with a title chip. Hand-rolled ANSI, no extra crate.
//!
//! Colors and spinners only on a TTY without NO_COLOR; otherwise the same layout in
//! plain characters, one final line per step. `--json` and the dashboard never use this
//! (their `Ui` is silent).

use std::io::{IsTerminal, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

/// What a piece of text is, for its color.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tone {
    /// White value text.
    Plain,
    /// Dim gray (labels, hints, absent values).
    Dim,
    /// Green (final, completed, OK, running).
    Good,
    /// Red (broken, expired, failed).
    Bad,
    /// Yellow (pending, open, in flight).
    Warn,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Style {
    pub color: bool,
    /// Redraw a spinner / progress line in place (TTY only).
    pub live: bool,
    /// Terminal width when known (TTY): boxes shrink long values to fit.
    pub width: Option<usize>,
}

pub const CHECK: &str = "✓";
pub const CROSS: &str = "✗";
const SPIN: [&str; 10] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];
const BAR_W: usize = 24;

impl Style {
    /// Color + live only on a TTY with NO_COLOR unset or empty.
    pub fn detect() -> Style {
        let tty = std::io::stdout().is_terminal();
        let no_color = std::env::var_os("NO_COLOR").is_some_and(|v| !v.is_empty());
        let color = tty && !no_color;
        let width = if tty { crossterm::terminal::size().ok().map(|(w, _)| w as usize).filter(|w| *w >= 40) } else { None };
        Style { color, live: color, width }
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub fn plain() -> Style {
        Style::default()
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub fn colored() -> Style {
        Style { color: true, live: false, width: None }
    }

    fn sgr(&self, code: &str, s: &str) -> String {
        if self.color && !s.is_empty() {
            format!("\x1b[{code}m{s}\x1b[0m")
        } else {
            s.to_string()
        }
    }

    pub fn paint(&self, t: Tone, s: &str) -> String {
        let code = match t {
            Tone::Plain => "97",
            Tone::Dim => "38;5;245",
            Tone::Good => "32",
            Tone::Bad => "31",
            Tone::Warn => "33",
        };
        self.sgr(code, s)
    }

    fn border(&self, s: &str) -> String {
        self.sgr("38;5;240", s)
    }

    pub fn bold(&self, s: &str) -> String {
        self.sgr("1;97", s)
    }

    /// Title chip: black on white with color, `[ title ]` without.
    pub fn chip(&self, title: &str) -> String {
        if self.color {
            format!("\x1b[30;107m {title} \x1b[0m")
        } else {
            format!("[ {title} ]")
        }
    }

    pub fn chip_width(&self, title: &str) -> usize {
        title.chars().count() + if self.color { 2 } else { 4 }
    }

    /// `✓ text` (green check, white text) or `✗ text` (red).
    pub fn step_line(&self, ok: bool, text: &str) -> String {
        if ok {
            format!("{} {}", self.paint(Tone::Good, CHECK), self.paint(Tone::Plain, text))
        } else {
            format!("{} {}", self.paint(Tone::Bad, CROSS), self.paint(Tone::Bad, text))
        }
    }

    /// Filled part green on a dark gray track (`█` / `░` without color).
    pub fn bar(&self, done: u64, total: u64, width: usize) -> String {
        let fill = if total == 0 { width } else { ((done.min(total) as u128 * width as u128) / total as u128) as usize };
        if self.color {
            format!("{}{}", self.sgr("32", &"█".repeat(fill)), self.sgr("38;5;237", &"█".repeat(width - fill)))
        } else {
            format!("{}{}", "█".repeat(fill), "░".repeat(width - fill))
        }
    }

    /// `✓ label  ████████  0.3s`.
    pub fn progress_line(&self, ok: Option<bool>, frame: usize, label: &str, done: u64, total: u64, elapsed: Duration) -> String {
        let icon = match ok {
            Some(true) => self.paint(Tone::Good, CHECK),
            Some(false) => self.paint(Tone::Bad, CROSS),
            None => self.paint(Tone::Dim, SPIN[frame % SPIN.len()]),
        };
        let text = if ok == Some(false) { self.paint(Tone::Bad, label) } else { self.paint(Tone::Plain, label) };
        format!("{icon} {text}  {}  {}", self.bar(done, total, BAR_W), self.paint(Tone::Dim, &elapsed_str(elapsed)))
    }
}

/// `0.3s`, `12.4s`, `2m 05s`.
pub fn elapsed_str(d: Duration) -> String {
    let s = d.as_secs_f64();
    if s < 60.0 {
        format!("{s:.1}s")
    } else {
        format!("{}m {:02}s", d.as_secs() / 60, d.as_secs() % 60)
    }
}

fn width(s: &str) -> usize {
    s.chars().count()
}

/// Middle ellipsis to at most `w` chars: `7xKXtg2C...osgAsU`.
pub fn ellipsize(s: &str, w: usize) -> String {
    let c: Vec<char> = s.chars().collect();
    if c.len() <= w || w < 5 {
        return s.to_string();
    }
    let keep = w - 3;
    let head = keep.div_ceil(2);
    let tail = keep - head;
    format!("{}...{}", c[..head].iter().collect::<String>(), c[c.len() - tail..].iter().collect::<String>())
}

// ---------------------------------------------------------------------------------------
// Boxes and tables

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Row {
    /// Dim label, value in its tone.
    Kv(String, String, Tone),
    /// Bold white sub-heading.
    Section(String),
    Blank,
    /// A full-width line of text in a tone.
    Text(String, Tone),
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Panel {
    pub chip: Option<String>,
    pub rows: Vec<Row>,
}

impl Panel {
    pub fn new(chip: &str) -> Self {
        Panel { chip: Some(chip.into()), rows: vec![] }
    }

    pub fn kv(mut self, label: &str, value: impl Into<String>) -> Self {
        let v: String = value.into();
        let tone = if v == "-" || v == "none" || v.is_empty() { Tone::Dim } else { Tone::Plain };
        self.rows.push(Row::Kv(label.into(), if v.is_empty() { "-".into() } else { v }, tone));
        self
    }

    pub fn kt(mut self, label: &str, value: impl Into<String>, tone: Tone) -> Self {
        self.rows.push(Row::Kv(label.into(), value.into(), tone));
        self
    }

    /// A status word, colored by [`status_tone`].
    pub fn state(self, label: &str, value: &str) -> Self {
        let t = status_tone(value);
        self.kt(label, value.to_uppercase(), t)
    }

    pub fn section(mut self, title: &str) -> Self {
        if !self.rows.is_empty() {
            self.rows.push(Row::Blank);
        }
        self.rows.push(Row::Section(title.into()));
        self
    }

    pub fn text(mut self, t: impl Into<String>, tone: Tone) -> Self {
        self.rows.push(Row::Text(t.into(), tone));
        self
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Table {
    pub chip: Option<String>,
    pub head: Vec<String>,
    /// Right-aligned columns (numbers).
    pub right: Vec<bool>,
    pub rows: Vec<Vec<(String, Tone)>>,
    /// Shown dim inside the box when there are no rows.
    pub empty: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Block {
    Panel(Panel),
    Table(Table),
    /// Dim gray line(s).
    Hint(String),
    /// A static `✓` / `✗` line.
    Step(bool, String),
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct View(pub Vec<Block>);

impl View {
    pub fn panel(p: Panel) -> View {
        View(vec![Block::Panel(p)])
    }

    pub fn with(mut self, b: Block) -> View {
        self.0.push(b);
        self
    }

    pub fn hint(self, h: impl Into<String>) -> View {
        self.with(Block::Hint(h.into()))
    }

    pub fn render(&self, st: &Style) -> String {
        let mut out = String::new();
        let mut prev_boxed = false;
        for (i, b) in self.0.iter().enumerate() {
            let boxed = matches!(b, Block::Panel(_) | Block::Table(_));
            if i > 0 {
                out.push('\n');
                if boxed || prev_boxed {
                    out.push('\n');
                }
            }
            match b {
                Block::Panel(p) => out += &render_panel(p, st),
                Block::Table(t) => out += &render_table(t, st),
                Block::Hint(h) => out += &h.lines().map(|l| st.paint(Tone::Dim, l)).collect::<Vec<_>>().join("\n"),
                Block::Step(ok, t) => out += &st.step_line(*ok, t),
            }
            prev_boxed = boxed;
        }
        out
    }
}

/// Green for done / healthy words, yellow for waiting ones, red for failures.
pub fn status_tone(s: &str) -> Tone {
    match s.to_ascii_lowercase().as_str() {
        "final" | "completed" | "ok" | "running" | "released" | "active" | "registered" | "connected" | "reachable" | "unlocked" | "synced" => Tone::Good,
        "pending" | "open" | "delivered" | "included" | "in flight" | "ok (in flight)" | "mineable" | "syncing" | "locked" => Tone::Warn,
        "expired" | "broken" | "failed" | "unreachable" | "offline" | "down" => Tone::Bad,
        "-" | "none" | "not connected" | "not registered" | "ended" | "cleared" | "not started" => Tone::Dim,
        _ => Tone::Plain,
    }
}

const PAD_X: usize = 3;
const GAP: usize = 4;

/// Box lines around already-painted content lines of plain width `inner`.
fn frame(lines: Vec<(String, usize)>, inner: usize, st: &Style) -> String {
    let full = inner + 2 * PAD_X;
    let side = st.border("│");
    let mut out = vec![st.border(&format!("╭{}╮", "─".repeat(full)))];
    let blank = format!("{side}{}{side}", " ".repeat(full));
    out.push(blank.clone());
    for (l, w) in lines {
        out.push(format!("{side}{}{l}{}{side}", " ".repeat(PAD_X), " ".repeat(inner.saturating_sub(w) + PAD_X)));
    }
    out.push(blank);
    out.push(st.border(&format!("╰{}╯", "─".repeat(full))));
    out.join("\n")
}

/// Room for content inside a box on this terminal, if the width is known.
fn max_inner(st: &Style) -> Option<usize> {
    st.width.map(|w| w.saturating_sub(2 + 2 * PAD_X + 1).max(20))
}

pub fn render_panel(p: &Panel, st: &Style) -> String {
    let lw = p.rows.iter().filter_map(|r| if let Row::Kv(l, ..) = r { Some(width(l)) } else { None }).max().unwrap_or(0);
    let mut vw = p.rows.iter().filter_map(|r| if let Row::Kv(_, v, _) = r { Some(width(v)) } else { None }).max().unwrap_or(0);
    let other = p
        .rows
        .iter()
        .filter_map(|r| match r {
            Row::Section(t) | Row::Text(t, _) => Some(width(t)),
            _ => None,
        })
        .chain(p.chip.as_ref().map(|c| st.chip_width(c)))
        .max()
        .unwrap_or(0);
    if let Some(m) = max_inner(st) {
        vw = vw.min(m.saturating_sub(lw + GAP).max(12));
    }
    let kvw = if vw > 0 { lw + GAP + vw } else { 0 };
    let inner = kvw.max(other.min(max_inner(st).unwrap_or(usize::MAX)));
    let mut lines = vec![];
    if let Some(c) = &p.chip {
        lines.push((st.chip(c), st.chip_width(c)));
        lines.push((String::new(), 0));
    }
    for r in &p.rows {
        lines.push(match r {
            Row::Kv(l, v, t) => {
                let v = ellipsize(v, vw);
                let w = lw + GAP + width(&v);
                (format!("{}{}{}", st.paint(Tone::Dim, l), " ".repeat(lw - width(l) + GAP), st.paint(*t, &v)), w)
            }
            Row::Section(t) => (st.bold(t), width(t)),
            Row::Blank => (String::new(), 0),
            Row::Text(t, tone) => {
                let t = ellipsize(t, inner);
                (st.paint(*tone, &t), width(&t))
            }
        });
    }
    frame(lines, inner, st)
}

pub fn render_table(t: &Table, st: &Style) -> String {
    const COL_GAP: usize = 3;
    let n = t.head.len();
    let mut w: Vec<usize> = t.head.iter().map(|h| width(h)).collect();
    for r in &t.rows {
        for (i, (c, _)) in r.iter().enumerate().take(n) {
            w[i] = w[i].max(width(c));
        }
    }
    let rowline = |cells: Vec<(String, Tone)>| -> (String, usize) {
        let mut s = String::new();
        let mut plain = 0;
        for (i, (c, tone)) in cells.into_iter().enumerate().take(n) {
            if i > 0 {
                s += &" ".repeat(COL_GAP);
                plain += COL_GAP;
            }
            let pad = " ".repeat(w[i] - width(&c));
            let painted = st.paint(tone, &c);
            if t.right.get(i).copied().unwrap_or(false) {
                s += &pad;
                s += &painted;
            } else {
                s += &painted;
                if i + 1 < n {
                    s += &pad;
                }
            }
            plain += if i + 1 < n || t.right.get(i).copied().unwrap_or(false) { w[i] } else { width(&c) };
        }
        (s, plain)
    };
    let mut lines = vec![];
    if let Some(c) = &t.chip {
        lines.push((st.chip(c), st.chip_width(c)));
        lines.push((String::new(), 0));
    }
    let table_w = w.iter().sum::<usize>() + COL_GAP * n.saturating_sub(1);
    let mut inner = table_w.max(t.chip.as_ref().map(|c| st.chip_width(c)).unwrap_or(0));
    if t.rows.is_empty() {
        inner = inner.max(width(&t.empty));
        lines.push((st.paint(Tone::Dim, &t.empty), width(&t.empty)));
    } else {
        lines.push(rowline(t.head.iter().map(|h| (h.clone(), Tone::Dim)).collect()));
        for r in &t.rows {
            lines.push(rowline(r.clone()));
        }
    }
    frame(lines, inner, st)
}

// ---------------------------------------------------------------------------------------
// Steps

/// Where a command's progress goes: nowhere (`--json`, the dashboard) or the terminal.
pub struct Ui {
    style: Option<Style>,
    lock: Arc<Mutex<()>>,
    /// A step already printed the error that ends the command.
    failed_shown: AtomicBool,
    /// Something was printed (the final box then gets a blank line before it).
    printed: AtomicBool,
}

impl Ui {
    pub fn silent() -> Ui {
        Ui { style: None, lock: Arc::new(Mutex::new(())), failed_shown: AtomicBool::new(false), printed: AtomicBool::new(false) }
    }

    pub fn plain(style: Style) -> Ui {
        Ui { style: Some(style), ..Ui::silent() }
    }

    pub fn is_plain(&self) -> bool {
        self.style.is_some()
    }

    pub fn style(&self) -> Style {
        self.style.unwrap_or_default()
    }

    pub fn failed_shown(&self) -> bool {
        self.failed_shown.load(Ordering::Relaxed)
    }

    pub fn printed(&self) -> bool {
        self.printed.load(Ordering::Relaxed)
    }

    fn print(&self, line: &str) {
        let _g = self.lock.lock();
        let mut o = std::io::stdout().lock();
        let _ = writeln!(o, "{line}");
        let _ = o.flush();
        self.printed.store(true, Ordering::Relaxed);
    }

    /// A step that already happened.
    pub fn done(&self, text: &str) {
        if let Some(st) = self.style {
            self.print(&st.step_line(true, text));
        }
    }

    pub fn hint(&self, text: &str) {
        if let Some(st) = self.style {
            self.print(&st.paint(Tone::Dim, text));
        }
    }

    /// Start a step: a spinner on a TTY until `ok` / `fail`.
    pub fn step(&self, label: impl Into<String>) -> Live<'_> {
        let state = Arc::new(Mutex::new(LiveState { label: label.into(), prog: None }));
        let stop = Arc::new(AtomicBool::new(false));
        let t0 = Instant::now();
        let th = match self.style {
            Some(st) if st.live => {
                let (state, stop, lock) = (state.clone(), stop.clone(), self.lock.clone());
                Some(std::thread::spawn(move || {
                    let mut frame = 0usize;
                    while !stop.load(Ordering::Relaxed) {
                        let line = {
                            let s = state.lock().unwrap();
                            live_line(&st, &s, frame, t0.elapsed())
                        };
                        {
                            let _g = lock.lock();
                            let mut o = std::io::stdout().lock();
                            let _ = write!(o, "\r\x1b[2K{line}");
                            let _ = o.flush();
                        }
                        frame += 1;
                        std::thread::sleep(Duration::from_millis(80));
                    }
                }))
            }
            _ => None,
        };
        Live { ui: self, state, stop, th, t0, finished: false }
    }
}

struct LiveState {
    label: String,
    prog: Option<(u64, u64)>,
}

fn live_line(st: &Style, s: &LiveState, frame: usize, el: Duration) -> String {
    // keep it on one row, or the in-place redraw breaks
    let room = st.width.map(|w| w.saturating_sub(BAR_W + 14)).unwrap_or(200);
    let label = ellipsize(&s.label, room.max(10));
    match s.prog {
        Some((d, t)) => st.progress_line(None, frame, &label, d, t, el),
        None => format!("{} {}", st.paint(Tone::Dim, SPIN[frame % SPIN.len()]), st.paint(Tone::Plain, &label)),
    }
}

pub struct Live<'a> {
    ui: &'a Ui,
    state: Arc<Mutex<LiveState>>,
    stop: Arc<AtomicBool>,
    th: Option<JoinHandle<()>>,
    t0: Instant,
    finished: bool,
}

impl Live<'_> {
    pub fn label(&self, s: impl Into<String>) {
        self.state.lock().unwrap().label = s.into();
    }

    /// Show the progress bar at `done / total`.
    pub fn progress(&self, done: u64, total: u64) {
        self.state.lock().unwrap().prog = Some((done, total));
    }

    /// A real event that happened during this wait: printed as its own `✓` line above.
    pub fn event(&self, text: &str) {
        let Some(st) = self.ui.style else { return };
        let _g = self.ui.lock.lock();
        let mut o = std::io::stdout().lock();
        let _ = writeln!(o, "{}{}", if st.live { "\r\x1b[2K" } else { "" }, st.step_line(true, text));
        let _ = o.flush();
        self.ui.printed.store(true, Ordering::Relaxed);
    }

    /// A failure event during the wait (`✗` line above); the step goes on.
    pub fn event_bad(&self, text: &str) {
        let Some(st) = self.ui.style else { return };
        let _g = self.ui.lock.lock();
        let mut o = std::io::stdout().lock();
        let _ = writeln!(o, "{}{}", if st.live { "\r\x1b[2K" } else { "" }, st.step_line(false, text));
        let _ = o.flush();
        self.ui.printed.store(true, Ordering::Relaxed);
    }

    fn finish(&mut self, ok: bool, text: &str) {
        self.finished = true;
        self.stop.store(true, Ordering::Relaxed);
        if let Some(t) = self.th.take() {
            let _ = t.join();
        }
        let Some(st) = self.ui.style else { return };
        let prog = self.state.lock().unwrap().prog;
        let line = match prog {
            Some((d, t)) if ok => st.progress_line(Some(true), 0, text, d.max(t), t, self.t0.elapsed()),
            Some((d, t)) => st.progress_line(Some(false), 0, text, d, t, self.t0.elapsed()),
            None => st.step_line(ok, text),
        };
        let _g = self.ui.lock.lock();
        let mut o = std::io::stdout().lock();
        let _ = writeln!(o, "{}{line}", if st.live { "\r\x1b[2K" } else { "" });
        let _ = o.flush();
        self.ui.printed.store(true, Ordering::Relaxed);
    }

    pub fn ok(mut self, text: impl AsRef<str>) {
        self.finish(true, text.as_ref());
    }

    /// `✗ label: error`, and the command's error is not printed a second time.
    pub fn fail(mut self, e: &anyhow::Error) {
        let label = self.state.lock().unwrap().label.clone();
        self.finish(false, &format!("{label}: {e:#}"));
        self.ui.failed_shown.store(true, Ordering::Relaxed);
    }

    /// Finish with the result of `r`: `ok(done(&v))` or `fail(e)`.
    pub fn wrap<T>(self, r: anyhow::Result<T>, done: impl FnOnce(&T) -> String) -> anyhow::Result<T> {
        match r {
            Ok(v) => {
                self.ok(done(&v));
                Ok(v)
            }
            Err(e) => {
                self.fail(&e);
                Err(e)
            }
        }
    }
}

impl Drop for Live<'_> {
    fn drop(&mut self) {
        if !self.finished {
            let label = self.state.lock().unwrap().label.clone();
            self.finish(false, &label);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn strip(s: &str) -> String {
        let mut out = String::new();
        let mut it = s.chars().peekable();
        while let Some(c) = it.next() {
            if c == '\x1b' {
                for d in it.by_ref() {
                    if d.is_ascii_alphabetic() {
                        break;
                    }
                }
            } else {
                out.push(c);
            }
        }
        out
    }

    fn sample_panel() -> Panel {
        Panel::new("Workspace info").kv("Name", "alpha").kv("ID", "7xKXtg2CW87d97TXJSDpbD5jBkheTqA83TZRuJosgAsU").state("State", "final").kv("Note", "")
    }

    #[test]
    fn panel_plain_layout() {
        let s = render_panel(&sample_panel(), &Style::plain());
        let lines: Vec<&str> = s.lines().collect();
        assert!(!s.contains('\x1b'), "no ANSI without color");
        assert!(lines[0].starts_with('╭') && lines[0].ends_with('╮'));
        assert!(lines.last().unwrap().starts_with('╰'));
        assert_eq!(lines[1].trim_matches('│').trim(), "");
        assert!(lines[2].starts_with("│   [ Workspace info ]"), "{s}");
        assert!(lines.iter().any(|l| l.contains("│   Name     alpha")), "label column then a 4-space gap: {s}");
        assert!(lines.iter().any(|l| l.contains("State    FINAL")));
        assert!(lines.iter().any(|l| l.contains("Note     -")));
        // every row is the same width
        let w = lines[0].chars().count();
        assert!(lines.iter().all(|l| l.chars().count() == w), "{s}");
    }

    #[test]
    fn panel_colored() {
        let st = Style::colored();
        let s = render_panel(&sample_panel(), &st);
        assert!(s.contains("\x1b[30;107m Workspace info \x1b[0m"), "white chip");
        assert!(s.contains("\x1b[32mFINAL\x1b[0m"), "green state");
        assert!(s.contains("\x1b[38;5;245mName\x1b[0m"), "dim label");
        assert!(s.contains("\x1b[97malpha\x1b[0m"), "white value");
        assert!(s.contains("\x1b[38;5;240m╭"), "gray border");
        let plain = strip(&s);
        let w = plain.lines().next().unwrap().chars().count();
        assert!(plain.lines().all(|l| l.chars().count() == w), "{plain}");
        // chip is 2 cols narrower with color than "[ x ]"
        assert!(plain.contains("│    Workspace info "));
    }

    #[test]
    fn panel_shrinks_to_terminal() {
        let st = Style { color: false, live: false, width: Some(40) };
        let s = render_panel(&sample_panel(), &st);
        assert!(s.lines().all(|l| l.chars().count() <= 40), "{s}");
        assert!(s.contains("..."), "long id is ellipsized: {s}");
    }

    #[test]
    fn table_render() {
        let t = Table {
            chip: Some("Jobs".into()),
            head: vec!["ID".into(), "STATUS".into(), "W".into()],
            right: vec![false, false, true],
            rows: vec![vec![("ab12".into(), Tone::Plain), ("completed".into(), Tone::Good), ("5".into(), Tone::Plain)], vec![("cd34".into(), Tone::Plain), ("open".into(), Tone::Warn), ("120.5".into(), Tone::Plain)]],
            empty: "no jobs".into(),
        };
        let p = render_table(&t, &Style::plain());
        assert!(p.contains("ID     STATUS          W"), "{p}");
        assert!(p.contains("ab12   completed       5"), "{p}");
        assert!(p.contains("cd34   open        120.5"), "{p}");
        let w = p.lines().next().unwrap().chars().count();
        assert!(p.lines().all(|l| l.chars().count() == w), "{p}");
        let c = render_table(&t, &Style::colored());
        assert!(c.contains("\x1b[38;5;245mSTATUS\x1b[0m"), "dim header");
        assert!(c.contains("\x1b[32mcompleted\x1b[0m") && c.contains("\x1b[33mopen\x1b[0m"));
        let e = render_table(&Table { rows: vec![], ..t }, &Style::plain());
        assert!(e.contains("no jobs") && !e.contains("STATUS"));
    }

    #[test]
    fn steps_and_progress() {
        let p = Style::plain();
        assert_eq!(p.step_line(true, "Connecting to repository"), "✓ Connecting to repository");
        assert_eq!(p.step_line(false, "Submit: refused"), "✗ Submit: refused");
        let c = Style::colored();
        assert_eq!(c.step_line(true, "Done"), "\x1b[32m✓\x1b[0m \x1b[97mDone\x1b[0m");
        assert!(c.step_line(false, "x").starts_with("\x1b[31m✗"));
        assert_eq!(p.bar(3, 6, 8), "████░░░░");
        assert_eq!(p.bar(9, 6, 4), "████");
        assert_eq!(p.bar(0, 0, 4), "████");
        assert_eq!(c.bar(1, 4, 4), "\x1b[32m█\x1b[0m\x1b[38;5;237m███\x1b[0m");
        assert_eq!(p.progress_line(Some(true), 0, "Opening browser", 1, 1, Duration::from_millis(300)), format!("✓ Opening browser  {}  0.3s", "█".repeat(24)));
        assert_eq!(elapsed_str(Duration::from_secs(125)), "2m 05s");
        assert_eq!(ellipsize("abcdefghijkl", 9), "abc...jkl");
        assert_eq!(ellipsize("abc", 2), "abc");
    }

    #[test]
    fn view_spacing_and_hint() {
        let v = View(vec![Block::Step(true, "Signed".into())]).with(Block::Panel(Panel::new("Tx").kv("A", "b"))).hint("pearl jobs");
        let s = v.render(&Style::plain());
        assert!(s.starts_with("✓ Signed\n\n╭"), "{s}");
        assert!(s.ends_with("╯\n\npearl jobs"), "{s}");
        let c = v.render(&Style::colored());
        assert!(c.ends_with("\x1b[38;5;245mpearl jobs\x1b[0m"));
    }

    #[test]
    fn status_tones() {
        assert_eq!(status_tone("final"), Tone::Good);
        assert_eq!(status_tone("COMPLETED"), Tone::Good);
        assert_eq!(status_tone("pending"), Tone::Warn);
        assert_eq!(status_tone("BROKEN"), Tone::Bad);
        assert_eq!(status_tone("none"), Tone::Dim);
    }

    #[test]
    fn silent_ui_prints_nothing_and_steps_are_safe() {
        let ui = Ui::silent();
        let s = ui.step("x");
        s.progress(1, 2);
        s.event("y");
        s.ok("z");
        assert!(!ui.printed() && !ui.failed_shown());
        let r: anyhow::Result<u32> = ui.step("a").wrap(Err(anyhow::anyhow!("boom")), |_| String::new());
        assert!(r.is_err() && ui.failed_shown());
    }
}
