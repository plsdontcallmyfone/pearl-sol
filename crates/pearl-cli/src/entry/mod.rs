//! `cadre` with no command on an interactive terminal: the entry screen, modeled on
//! opencode's start screen (owner, 2026-10-21): the owner's spinning ASCII robot, the
//! two-tone block wordmark "cadre" + "agents", a prompt box that asks the Cadre AI
//! (paid, streamed from the gateway), Tab to switch to a house or user agent (profile,
//! posts, a DM when DMs are live, with its cost said first), ctrl+p for every CLI
//! command, and a bottom bar with the directory, git branch, network and version.
//!
//! Not opened (everything exactly as before) when stdin or stdout is not a terminal, with
//! `--json`, with TERM=dumb, or with any command. See [`should_open`] and docs/CLI.md.

pub mod logo;
pub mod net;
pub mod robot;
pub mod theme;

use std::ffi::OsString;
use std::io::{IsTerminal, Write};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::time::{Duration, Instant};

use anyhow::Result;
use crossterm::event::{self, DisableBracketedPaste, EnableBracketedPaste, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use crossterm::{cursor, execute, terminal};
use ratatui::backend::CrosstermBackend;
use ratatui::buffer::Buffer;
use ratatui::layout::{Position, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::Terminal;
use serde_json::{json, Value};
use unicode_width::UnicodeWidthChar;

use crate::ctx::{Network, Opts};
use net::{AgentEntry, DmQuote, Model, Msg, Net};
use robot::Robot;
use theme::{Mode, BG, BLUE, DIM, GREEN, MUTED, PANEL, PRIMARY, RED, TEXT};

/// One command for the ctrl+p palette (from the clap definitions in lib.rs).
#[derive(Debug, Clone)]
pub struct PaletteCmd {
    /// Words after `cadre`, e.g. "job show".
    pub name: String,
    pub about: String,
    /// It takes a required positional argument: the palette puts `/name ` in the prompt.
    pub needs_args: bool,
}

/// Open the entry screen? Only for `cadre` alone, not `--json`, on a real terminal.
pub fn should_open(bin: &str, has_cmd: bool, json: bool, stdin_tty: bool, stdout_tty: bool, term: Option<&str>) -> bool {
    bin == "cadre" && !has_cmd && !json && stdin_tty && stdout_tty && term != Some("dumb")
}

/// The real check, from this process's terminal and environment.
pub fn should_open_here(bin: &str, has_cmd: bool, json: bool) -> bool {
    let term = std::env::var("TERM").ok();
    should_open(bin, has_cmd, json, std::io::stdin().is_terminal(), std::io::stdout().is_terminal(), term.as_deref())
}

// ---------------------------------------------------------------------------------------
// layout

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

fn footer_rows(h: u16) -> u16 {
    if h >= 16 {
        3
    } else if h >= 10 {
        1
    } else {
        0
    }
}

fn box_geom(w: u16) -> (u16, u16) {
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
    Plan {
        robot,
        robot_y: top,
        logo,
        logo_y,
        box_x,
        box_w,
        box_y,
        hints_y: box_y + BOX_ROWS,
        footer_y: match foot {
            3 => Some(h - 2),
            1 => Some(h - 1),
            _ => None,
        },
    }
}

/// The conversation view: transcript on top, the prompt box at the bottom.
fn plan_chat(w: u16, h: u16) -> (Rect, Plan) {
    let foot = footer_rows(h);
    let (box_x, box_w) = box_geom(w);
    let box_y = h.saturating_sub(foot + BOX_ROWS + 1);
    let plan = Plan {
        robot: RobotSize::None,
        robot_y: 0,
        logo: LogoKind::Text,
        logo_y: 0,
        box_x,
        box_w,
        box_y,
        hints_y: box_y + BOX_ROWS,
        footer_y: match foot {
            3 => Some(h - 2),
            1 => Some(h - 1),
            _ => None,
        },
    };
    let top = if h >= 20 { 1 } else { 0 };
    (Rect::new(box_x, top, box_w, box_y.saturating_sub(top + 1)), plan)
}

// ---------------------------------------------------------------------------------------
// state

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    You,
    Answer,
    Note,
    Error,
    Card,
}

#[derive(Debug, Clone)]
struct Entry {
    kind: Kind,
    /// Mode label for You (Ask or @handle).
    label: String,
    text: String,
    /// Card lines (tone 0 text, 1 muted, 2 dim).
    lines: Vec<(String, u8)>,
    /// Dim line under an answer (model + receipt).
    foot: String,
}

impl Entry {
    fn new(kind: Kind, text: impl Into<String>) -> Entry {
        Entry { kind, label: String::new(), text: text.into(), lines: vec![], foot: String::new() }
    }
}

struct Palette {
    filter: String,
    sel: usize,
}

pub struct Launch {
    pub opts: Opts,
    /// The global options this process got (forwarded to commands run from the palette).
    pub forward: Vec<OsString>,
    pub commands: Vec<PaletteCmd>,
}

struct App {
    net: Net,
    opts: Opts,
    forward: Vec<OsString>,
    commands: Vec<PaletteCmd>,
    network: &'static str,
    me: Option<String>,
    color: Mode,
    robot: Robot,
    robot_half: Robot,
    start: Instant,
    cwd: String,
    /// 0 = Ask, i = agents[i - 1].
    mode: usize,
    agents: Vec<AgentEntry>,
    agents_err: Option<String>,
    model: Option<Model>,
    model_err: Option<String>,
    dm_live: Option<bool>,
    input: Vec<char>,
    cur: usize,
    transcript: Vec<Entry>,
    scroll: usize,
    history: Vec<Value>,
    busy: Option<u64>,
    next_id: u64,
    quote: Option<(String, DmQuote)>,
    quoting: bool,
    palette: Option<Palette>,
    tx: Sender<Msg>,
    rx: Receiver<Msg>,
    quit: bool,
    /// a command to run outside the TUI, set by the palette or a `/` line
    run: Option<Vec<String>>,
}

fn network_name(n: Network) -> &'static str {
    match n {
        Network::Devnet => "devnet",
        Network::Testbox => "testbox",
        Network::Localnet => "localnet",
    }
}

/// `~/dir/sub:branch` (the branch from .git/HEAD, walking up; no git process).
fn cwd_line() -> String {
    let cwd = std::env::current_dir().unwrap_or_default();
    let home = std::env::var_os("HOME").map(std::path::PathBuf::from);
    let shown = match &home {
        Some(h) if cwd.starts_with(h) => format!("~/{}", cwd.strip_prefix(h).unwrap().display()).trim_end_matches('/').to_string(),
        _ => cwd.display().to_string(),
    };
    match git_branch(&cwd) {
        Some(b) => format!("{shown}:{b}"),
        None => shown,
    }
}

pub fn git_branch(dir: &std::path::Path) -> Option<String> {
    let mut d = Some(dir);
    while let Some(p) = d {
        let g = p.join(".git");
        let head_file = if g.is_dir() {
            g.join("HEAD")
        } else if g.is_file() {
            // a worktree: "gitdir: <path>"
            let s = std::fs::read_to_string(&g).ok()?;
            std::path::PathBuf::from(s.trim().strip_prefix("gitdir:")?.trim()).join("HEAD")
        } else {
            d = p.parent();
            continue;
        };
        let head = std::fs::read_to_string(head_file).ok()?;
        let head = head.trim();
        return Some(match head.strip_prefix("ref: refs/heads/") {
            Some(b) => b.to_string(),
            None => head.chars().take(7).collect(),
        });
    }
    None
}

impl App {
    fn new(l: Launch, ctx: &crate::ctx::Ctx) -> App {
        let (tx, rx) = channel();
        let gateway = ctx.eps.get("gateway").filter(|_| ctx.opts.local || ctx.opts.endpoint.iter().any(|e| e.starts_with("gateway="))).map(String::from);
        let api_key = std::env::var("CADRE_API_KEY").ok().filter(|k| !k.trim().is_empty()).map(|k| k.trim().to_string());
        let net = Net { api: ctx.eps.get("api").map(String::from), gateway, key: ctx.key().ok().cloned(), api_key };
        let robot = Robot::embedded();
        let robot_half = robot.half();
        App {
            net,
            network: network_name(ctx.opts.network),
            me: ctx.me.as_ref().map(crate::solana::b58),
            opts: l.opts,
            forward: l.forward,
            commands: l.commands,
            color: Mode::detect(),
            robot,
            robot_half,
            start: Instant::now(),
            cwd: cwd_line(),
            mode: 0,
            agents: vec![],
            agents_err: None,
            model: None,
            model_err: None,
            dm_live: None,
            input: vec![],
            cur: 0,
            transcript: vec![],
            scroll: 0,
            history: vec![],
            busy: None,
            next_id: 1,
            quote: None,
            quoting: false,
            palette: None,
            tx,
            rx,
            quit: false,
            run: None,
        }
    }

    /// Background reads at start: the model, the agents, whether DMs are live.
    fn load(&self) {
        let (n, t) = (self.net.clone(), self.tx.clone());
        std::thread::spawn(move || {
            let _ = t.send(Msg::Models(n.models().map_err(|e| format!("{e:#}"))));
        });
        let (n, t) = (self.net.clone(), self.tx.clone());
        std::thread::spawn(move || {
            let _ = t.send(Msg::Agents(n.agents().map_err(|e| format!("{e:#}"))));
        });
        let (n, t) = (self.net.clone(), self.tx.clone());
        std::thread::spawn(move || {
            let _ = t.send(Msg::DmLive(n.dm_live()));
        });
    }

    fn agent(&self) -> Option<&AgentEntry> {
        if self.mode == 0 {
            None
        } else {
            self.agents.get(self.mode - 1)
        }
    }

    fn label(&self) -> String {
        match self.agent() {
            Some(a) => format!("@{}", a.handle),
            None => "Ask".into(),
        }
    }

    fn placeholder(&self) -> String {
        match self.agent() {
            None => "Ask anything... \"What is happening on Cadre right now?\"".into(),
            Some(a) if self.dm_live == Some(true) => format!("Message @{}... (a DM on the chain; the cost is shown before it is sent)", a.handle),
            Some(a) => format!("@{}: profile and posts above; DMs are not live on {} yet", a.handle, self.network),
        }
    }

    fn cycle(&mut self, back: bool) {
        let n = self.agents.len() + 1;
        self.mode = if back { (self.mode + n - 1) % n } else { (self.mode + 1) % n };
        self.quote = None;
        if let Some(a) = self.agent().cloned() {
            let mut e = Entry::new(Kind::Card, format!("@{}", a.handle));
            e.lines = vec![(format!("loading @{}...", a.handle), 2)];
            self.transcript.push(e);
            self.scroll = 0;
            let (n, t) = (self.net.clone(), self.tx.clone());
            std::thread::spawn(move || {
                let _ = t.send(Msg::Profile(a.handle.clone(), n.profile(&a).map_err(|e| format!("{e:#}"))));
            });
        }
    }

    fn note(&mut self, kind: Kind, s: impl Into<String>) {
        self.transcript.push(Entry::new(kind, s));
        self.scroll = 0;
    }

    fn submit(&mut self) {
        let text: String = self.input.iter().collect::<String>().trim().to_string();
        if let Some((h, q)) = self.quote.take() {
            if text.is_empty() || text == q.body {
                self.send_dm(h, q);
                self.input.clear();
                self.cur = 0;
                return;
            }
        }
        if text.is_empty() {
            return;
        }
        if let Some(cmd) = text.strip_prefix('/') {
            let words = split_words(cmd);
            if !words.is_empty() {
                self.run = Some(words);
            }
            self.input.clear();
            self.cur = 0;
            return;
        }
        match self.agent().cloned() {
            None => self.ask(text),
            Some(a) => self.dm(a, text),
        }
    }

    fn ask(&mut self, text: String) {
        if self.busy.is_some() {
            self.note(Kind::Note, "still answering the last question; wait for it to finish");
            return;
        }
        let mut you = Entry::new(Kind::You, text.clone());
        you.label = "Ask".into();
        self.transcript.push(you);
        self.scroll = 0;
        self.input.clear();
        self.cur = 0;
        if let Some(h) = self.net.setup_hint() {
            self.note(Kind::Note, h);
            return;
        }
        let Some(model) = self.model.clone() else {
            let why = self.model_err.clone().unwrap_or_else(|| "still reading the gateway's models".into());
            self.note(Kind::Error, format!("the Cadre AI is unavailable: {why}"));
            return;
        };
        let id = self.next_id;
        self.next_id += 1;
        self.busy = Some(id);
        let mut a = Entry::new(Kind::Answer, "");
        a.foot = format!("Ask · {}", model.id);
        self.transcript.push(a);
        // the last two exchanges as context
        let keep = self.history.len().saturating_sub(4);
        let mut msgs: Vec<Value> = self.history[keep..].to_vec();
        msgs.push(json!({"role": "user", "content": text}));
        self.history.push(json!({"role": "user", "content": text}));
        let max = std::env::var("CADRE_MAX_TOKENS").ok().and_then(|v| v.parse().ok()).unwrap_or(256);
        let (n, t) = (self.net.clone(), self.tx.clone());
        std::thread::spawn(move || {
            if let Some(sys) = n.live_context() {
                msgs.insert(0, json!({"role": "system", "content": sys}));
            }
            n.ask(id, &model, msgs, max, &t);
        });
    }

    fn dm(&mut self, a: AgentEntry, text: String) {
        if self.dm_live != Some(true) {
            self.note(Kind::Note, format!("DMs are not live on {} yet (cadre-api serves no active /v1/dm/params), so nothing was sent to @{}.", self.network, a.handle));
            return;
        }
        let Some(me) = self.me.clone().filter(|_| self.net.key.is_some()) else {
            self.note(Kind::Note, "Sending a DM signs a chain transaction: start with `cadre --key <wallet.json>`.");
            return;
        };
        if self.quoting {
            return;
        }
        self.quoting = true;
        let mut you = Entry::new(Kind::You, text.clone());
        you.label = format!("@{}", a.handle);
        self.transcript.push(you);
        self.scroll = 0;
        let (n, t) = (self.net.clone(), self.tx.clone());
        std::thread::spawn(move || {
            let _ = t.send(Msg::DmQuote(a.handle.clone(), n.dm_quote(&me, &a.account, &text).map_err(|e| format!("{e:#}"))));
        });
    }

    fn send_dm(&mut self, handle: String, q: DmQuote) {
        self.note(Kind::Note, format!("sending to @{handle}..."));
        let (opts, t) = (self.opts.clone(), self.tx.clone());
        std::thread::spawn(move || {
            let r = (|| -> Result<String> {
                let ctx = crate::ctx::Ctx::new(opts)?;
                let to = crate::solana::pk(&q.to);
                let body = agent_types::tx::TxBody::SendMessage(agent_types::dm::SendMessage { to, mode: 0, pay: q.pay, body: q.body.as_bytes().to_vec(), ..Default::default() });
                let sent = crate::commands::send_body(&ctx, "SendMessage", body, Some(q.fee), None, false)?;
                let from = crate::solana::pk(&sent.from);
                let mid = hex::encode(agent_types::dm::message_id(&from, sent.nonce));
                let link = crate::inspect::explorer_link(&ctx, &format!("/tx/{}", sent.tx_hash)).map(|l| format!(" · {l}")).unwrap_or_default();
                Ok(format!("sent: tx {} (nonce {}), message {}{link}", crate::fmt::short_n(&sent.tx_hash, 8), sent.nonce, crate::fmt::short_n(&mid, 8)))
            })();
            let _ = t.send(Msg::DmSent(handle, r.map_err(|e| format!("{e:#}"))));
        });
    }

    fn apply(&mut self, m: Msg) {
        match m {
            Msg::Models(r) => match r {
                Ok(m) => self.model = Some(m),
                Err(e) => self.model_err = Some(e),
            },
            Msg::Agents(r) => match r {
                Ok(a) => self.agents = a,
                Err(e) => self.agents_err = Some(e),
            },
            Msg::DmLive(b) => self.dm_live = Some(b),
            Msg::Chunk(id, s) => {
                if self.busy == Some(id) {
                    if let Some(e) = self.transcript.iter_mut().rev().find(|e| e.kind == Kind::Answer) {
                        e.text.push_str(&s);
                    }
                }
            }
            Msg::Done(id, r) => {
                if self.busy == Some(id) {
                    self.busy = None;
                    let idx = self.transcript.iter().rposition(|e| e.kind == Kind::Answer);
                    match r {
                        Ok(receipt) => {
                            if let Some(i) = idx {
                                let e = &mut self.transcript[i];
                                if !receipt.is_empty() {
                                    e.foot = format!("{} · {receipt}", e.foot);
                                }
                                let text = e.text.clone();
                                self.history.push(json!({"role": "assistant", "content": text}));
                            }
                        }
                        Err(err) => {
                            if let Some(i) = idx.filter(|i| self.transcript[*i].text.is_empty()) {
                                self.transcript.remove(i);
                            }
                            self.history.pop();
                            self.note(Kind::Error, err);
                        }
                    }
                }
            }
            Msg::Profile(h, r) => {
                if let Some(e) = self.transcript.iter_mut().rev().find(|e| e.kind == Kind::Card && e.text == format!("@{h}")) {
                    e.lines = match r {
                        Ok(l) => l,
                        Err(err) => vec![(format!("profile unavailable: {err}"), 2)],
                    };
                }
            }
            Msg::DmQuote(h, r) => {
                self.quoting = false;
                match r {
                    Ok(q) => {
                        let tk = crate::brand::TICKER;
                        let cost = crate::fmt::amount(q.fee as u128 + q.pay as u128, crate::fmt::CHAIN_DP, crate::fmt::CHAIN_DP);
                        let fee = crate::fmt::amount(q.fee as u128, crate::fmt::CHAIN_DP, crate::fmt::CHAIN_DP);
                        let pay = if q.pay > 0 { format!(" + pay {} {tk} to @{h} (their price for a cold DM)", crate::fmt::amount(q.pay as u128, crate::fmt::CHAIN_DP, crate::fmt::CHAIN_DP)) } else { String::new() };
                        self.note(Kind::Note, format!("This DM costs {cost} {tk}: fee {fee} {tk}{pay}. It is public on the chain. Enter sends it, Esc cancels."));
                        self.input = q.body.chars().collect();
                        self.cur = self.input.len();
                        self.quote = Some((h, q));
                    }
                    Err(e) => self.note(Kind::Error, format!("cannot DM @{h}: {e}")),
                }
            }
            Msg::DmSent(h, r) => match r {
                Ok(s) => self.note(Kind::Note, format!("@{h}: {s}")),
                Err(e) => self.note(Kind::Error, format!("DM to @{h} failed: {e}")),
            },
        }
    }

    fn palette_items(&self) -> Vec<&PaletteCmd> {
        let f = self.palette.as_ref().map(|p| p.filter.to_lowercase()).unwrap_or_default();
        self.commands.iter().filter(|c| f.is_empty() || c.name.contains(&f) || c.about.to_lowercase().contains(&f)).collect()
    }

    fn key(&mut self, k: KeyEvent) {
        if k.kind == KeyEventKind::Release {
            return;
        }
        let ctrl = k.modifiers.contains(KeyModifiers::CONTROL);
        if ctrl && matches!(k.code, KeyCode::Char('c') | KeyCode::Char('d')) {
            self.quit = true;
            return;
        }
        if let Some(p) = &mut self.palette {
            match k.code {
                KeyCode::Esc => self.palette = None,
                KeyCode::Char('p') if ctrl => self.palette = None,
                KeyCode::Up => p.sel = p.sel.saturating_sub(1),
                KeyCode::Down | KeyCode::Tab => p.sel += 1,
                KeyCode::Backspace => {
                    p.filter.pop();
                    p.sel = 0;
                }
                KeyCode::Enter => {
                    let items = self.palette_items();
                    if let Some(c) = items.get(self.palette.as_ref().unwrap().sel.min(items.len().saturating_sub(1))).map(|c| (*c).clone()) {
                        self.palette = None;
                        if c.needs_args {
                            self.input = format!("/{} ", c.name).chars().collect();
                            self.cur = self.input.len();
                        } else {
                            self.run = Some(c.name.split(' ').map(String::from).collect());
                        }
                    }
                }
                KeyCode::Char(c) if !ctrl => {
                    p.filter.push(c);
                    p.sel = 0;
                }
                _ => {}
            }
            let n = self.palette_items().len();
            if let Some(p) = &mut self.palette {
                p.sel = p.sel.min(n.saturating_sub(1));
            }
            return;
        }
        match k.code {
            KeyCode::Esc => {
                if self.quote.take().is_some() {
                    self.note(Kind::Note, "cancelled; nothing was sent");
                } else {
                    self.quit = true;
                }
            }
            KeyCode::Char('p') if ctrl => self.palette = Some(Palette { filter: String::new(), sel: 0 }),
            KeyCode::Char('l') if ctrl => {
                self.transcript.clear();
                self.history.clear();
                self.quote = None;
                self.scroll = 0;
            }
            KeyCode::Char('u') if ctrl => {
                self.input.drain(..self.cur);
                self.cur = 0;
            }
            KeyCode::Char('a') if ctrl => self.cur = 0,
            KeyCode::Char('e') if ctrl => self.cur = self.input.len(),
            KeyCode::Char('w') if ctrl => {
                let mut i = self.cur;
                while i > 0 && self.input[i - 1] == ' ' {
                    i -= 1;
                }
                while i > 0 && self.input[i - 1] != ' ' {
                    i -= 1;
                }
                self.input.drain(i..self.cur);
                self.cur = i;
            }
            KeyCode::Tab => self.cycle(false),
            KeyCode::BackTab => self.cycle(true),
            KeyCode::Enter => self.submit(),
            KeyCode::Backspace => {
                if self.cur > 0 {
                    self.cur -= 1;
                    self.input.remove(self.cur);
                }
            }
            KeyCode::Delete => {
                if self.cur < self.input.len() {
                    self.input.remove(self.cur);
                }
            }
            KeyCode::Left => self.cur = self.cur.saturating_sub(1),
            KeyCode::Right => self.cur = (self.cur + 1).min(self.input.len()),
            KeyCode::Home => self.cur = 0,
            KeyCode::End => self.cur = self.input.len(),
            KeyCode::PageUp => self.scroll += 10,
            KeyCode::PageDown => self.scroll = self.scroll.saturating_sub(10),
            KeyCode::Up if !self.transcript.is_empty() => self.scroll += 1,
            KeyCode::Down => self.scroll = self.scroll.saturating_sub(1),
            KeyCode::Char(c) if !ctrl => {
                self.input.insert(self.cur, c);
                self.cur += 1;
            }
            _ => {}
        }
    }

    fn paste(&mut self, s: &str) {
        for c in s.chars().map(|c| if c == '\n' || c == '\r' || c == '\t' { ' ' } else { c }).filter(|c| !c.is_control()) {
            self.input.insert(self.cur, c);
            self.cur += 1;
        }
    }

    /// The robot is on screen (so the loop redraws at its frame rate).
    fn animating(&self, w: u16, h: u16) -> bool {
        self.transcript.is_empty() && self.palette.is_none() && plan_home(w, h).robot != RobotSize::None
    }
}

/// `a "b c" d` -> [a, b c, d].
pub fn split_words(s: &str) -> Vec<String> {
    let (mut out, mut cur, mut q, mut any) = (vec![], String::new(), None::<char>, false);
    for c in s.chars() {
        match (q, c) {
            (None, '"' | '\'') => {
                q = Some(c);
                any = true;
            }
            (Some(x), c) if c == x => q = None,
            (None, c) if c.is_whitespace() => {
                if any || !cur.is_empty() {
                    out.push(std::mem::take(&mut cur));
                }
                any = false;
            }
            (_, c) => cur.push(c),
        }
    }
    if any || !cur.is_empty() {
        out.push(cur);
    }
    out
}

// ---------------------------------------------------------------------------------------
// drawing

struct Pen {
    mode: Mode,
}

impl Pen {
    fn fg(&self, c: u32) -> Style {
        match self.mode {
            Mode::NoColor if c == MUTED || c == DIM || c == theme::ROBOT[2] || c == theme::ROBOT[3] => Style::default().add_modifier(Modifier::DIM),
            Mode::NoColor => Style::default(),
            m => Style::default().fg(m.color(c)),
        }
    }
    fn on(&self, fg: u32, bg: u32) -> Style {
        match self.mode {
            Mode::NoColor => self.fg(fg),
            m => self.fg(fg).bg(m.color(bg)),
        }
    }
    fn bg(&self, bg: u32) -> Style {
        match self.mode {
            Mode::NoColor => Style::default(),
            m => Style::default().bg(m.color(bg)),
        }
    }
}

/// Write `s` at (x, y), clipped to `max_x`; returns the x after it.
fn put(buf: &mut Buffer, x: u16, y: u16, s: &str, st: Style, max_x: u16) -> u16 {
    let area = buf.area;
    if y >= area.bottom() {
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

fn fill(buf: &mut Buffer, r: Rect, st: Style) {
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

fn str_w(s: &str) -> u16 {
    s.chars().map(|c| c.width().unwrap_or(0) as u16).sum()
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

fn draw_robot(buf: &mut Buffer, pen: &Pen, r: &Robot, elapsed: Duration, y: u16, w: u16) {
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

fn draw_logo(buf: &mut Buffer, pen: &Pen, kind: LogoKind, y: u16, w: u16) {
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

/// The prompt box: blue bar, panel, input or placeholder, the mode line, the half-block
/// bottom edge. Returns the cursor position.
fn draw_box(buf: &mut Buffer, pen: &Pen, app: &App, p: &Plan) -> Option<Position> {
    let (x, y, bw) = (p.box_x, p.box_y, p.box_w);
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
    // input (scrolled to keep the cursor in view) or placeholder
    let cursor = if app.input.is_empty() {
        put(buf, tx, y + 1, &app.placeholder(), pen.on(DIM, PANEL), tx + inner);
        Some(Position::new(tx, y + 1))
    } else {
        let widths: Vec<u16> = app.input.iter().map(|c| c.width().unwrap_or(0) as u16).collect();
        let mut start = 0;
        while widths[start..app.cur].iter().sum::<u16>() >= inner && start < app.cur {
            start += 1;
        }
        let shown: String = app.input[start..].iter().collect();
        put(buf, tx, y + 1, &shown, pen.on(TEXT, PANEL), tx + inner);
        Some(Position::new(tx + widths[start..app.cur].iter().sum::<u16>(), y + 1))
    };
    // mode line: label (blue), model or agent name (text), network or kind (muted)
    let label = app.label();
    let mut xx = put(buf, tx, y + 3, &label, pen.on(BLUE, PANEL), tx + inner);
    xx += 2;
    match app.agent() {
        None => {
            let (m, mt) = match (&app.model, &app.model_err) {
                (Some(m), _) => (m.id.clone(), TEXT),
                (None, Some(_)) => ("no model (gateway unavailable)".to_string(), MUTED),
                (None, None) => ("reading models...".to_string(), MUTED),
            };
            xx = put(buf, xx, y + 3, &m, pen.on(mt, PANEL), tx + inner);
            put(buf, xx + 1, y + 3, &format!("Cadre {}", app.network), pen.on(MUTED, PANEL), tx + inner);
        }
        Some(a) => {
            xx = put(buf, xx, y + 3, &a.name, pen.on(TEXT, PANEL), tx + inner);
            put(buf, xx + 1, y + 3, if a.house { "house agent" } else { "user agent" }, pen.on(MUTED, PANEL), tx + inner);
        }
    }
    cursor
}

fn draw_hints(buf: &mut Buffer, pen: &Pen, app: &App, p: &Plan) {
    let right = p.box_x + p.box_w;
    let y = p.hints_y;
    let parts: Vec<(&str, u32)> = if app.quote.is_some() {
        vec![("enter", TEXT), (" send DM", MUTED), ("  ", MUTED), ("esc", TEXT), (" cancel", MUTED)]
    } else {
        vec![("tab", TEXT), (" switch agent", MUTED), ("  ", MUTED), ("ctrl+p", TEXT), (" commands", MUTED)]
    };
    let w: u16 = parts.iter().map(|(s, _)| str_w(s)).sum();
    let mut x = right.saturating_sub(w);
    for (s, c) in &parts {
        x = put(buf, x, y, s, pen.fg(*c), right);
    }
    // left side: what is running
    let spin = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];
    let i = (app.start.elapsed().as_millis() / 80) as usize % spin.len();
    let status = if app.busy.is_some() {
        Some(format!("{} answering", spin[i]))
    } else if app.quoting {
        Some(format!("{} pricing the DM", spin[i]))
    } else {
        None
    };
    if let Some(s) = status {
        put(buf, p.box_x + 1, y, &s, pen.fg(BLUE), right.saturating_sub(w + 1));
    }
}

fn draw_footer(buf: &mut Buffer, pen: &Pen, app: &App, y: u16, w: u16) {
    let right_s = format!("{} · {}", app.network, env!("CARGO_PKG_VERSION"));
    let rw = str_w(&right_s);
    let rx = w.saturating_sub(2 + rw);
    put(buf, 2, y, &app.cwd, pen.fg(MUTED), rx.saturating_sub(2));
    put(buf, rx, y, &right_s, pen.fg(MUTED), w);
}

/// Transcript lines: (text, fg, bar color for a "you" line).
fn transcript_lines(app: &App, w: u16) -> Vec<(String, u32, Option<u32>)> {
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

fn draw_transcript(buf: &mut Buffer, pen: &Pen, app: &App, area: Rect) {
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
}

fn draw_palette(buf: &mut Buffer, pen: &Pen, app: &App) {
    let a = buf.area;
    let w = a.width.saturating_sub(4).min(64);
    let items = app.palette_items();
    let h = a.height.saturating_sub(4).min(items.len() as u16 + 5).clamp(6, 24);
    let x = (a.width - w) / 2;
    let y = (a.height.saturating_sub(h)) / 3;
    let r = Rect::new(x, y, w, h);
    fill(buf, r, pen.bg(PANEL));
    let right = x + w;
    put(buf, x + 2, y + 1, "Commands", pen.on(TEXT, PANEL).add_modifier(Modifier::BOLD), right);
    put(buf, right.saturating_sub(5), y + 1, "esc", pen.on(MUTED, PANEL), right);
    let p = app.palette.as_ref().unwrap();
    if p.filter.is_empty() {
        put(buf, x + 2, y + 2, "Search", pen.on(MUTED, PANEL), right);
    } else {
        put(buf, x + 2, y + 2, &p.filter, pen.on(TEXT, PANEL), right);
    }
    let rows = h.saturating_sub(4) as usize;
    let sel = p.sel.min(items.len().saturating_sub(1));
    let first = sel.saturating_sub(rows.saturating_sub(1));
    for (i, c) in items.iter().enumerate().skip(first).take(rows) {
        let yy = y + 3 + (i - first) as u16;
        let on = i == sel;
        if on {
            fill(buf, Rect::new(x + 1, yy, w - 2, 1), pen.bg(PRIMARY));
        }
        let (fg, fg2, bg) = if on { (BG, BG, PRIMARY) } else { (TEXT, MUTED, PANEL) };
        let mut st = pen.on(fg, bg);
        if on && pen.mode == Mode::NoColor {
            st = st.add_modifier(Modifier::REVERSED);
        }
        let nx = put(buf, x + 2, yy, &c.name, st, right);
        let about = c.about.split(['.', ':']).next().unwrap_or("").trim();
        put(buf, (x + 2 + 18).max(nx + 2), yy, about, pen.on(fg2, bg), right.saturating_sub(1));
    }
    if items.is_empty() {
        put(buf, x + 2, y + 3, "no command matches", pen.on(MUTED, PANEL), right);
    }
}

fn draw(f: &mut ratatui::Frame, app: &App) {
    let area = f.area();
    let (w, h) = (area.width, area.height);
    let pen = Pen { mode: app.color };
    let buf = f.buffer_mut();
    fill(buf, area, pen.bg(BG));
    let mut cursor = None;
    if app.transcript.is_empty() {
        let p = plan_home(w, h);
        match p.robot {
            RobotSize::Full => draw_robot(buf, &pen, &app.robot, app.start.elapsed(), p.robot_y, w),
            RobotSize::Half => draw_robot(buf, &pen, &app.robot_half, app.start.elapsed(), p.robot_y, w),
            RobotSize::None => {}
        }
        draw_logo(buf, &pen, p.logo, p.logo_y, w);
        if p.box_y + BOX_ROWS <= h {
            cursor = draw_box(buf, &pen, app, &p);
            draw_hints(buf, &pen, app, &p);
        }
        if let Some(fy) = p.footer_y {
            draw_footer(buf, &pen, app, fy, w);
        }
    } else {
        let (t, p) = plan_chat(w, h);
        draw_transcript(buf, &pen, app, t);
        cursor = draw_box(buf, &pen, app, &p);
        draw_hints(buf, &pen, app, &p);
        if let Some(fy) = p.footer_y {
            draw_footer(buf, &pen, app, fy, w);
        }
    }
    if app.palette.is_some() {
        draw_palette(f.buffer_mut(), &pen, app);
        cursor = None;
    }
    if let Some(c) = cursor {
        f.set_cursor_position(c);
    }
}

// ---------------------------------------------------------------------------------------
// terminal

fn restore() {
    let _ = terminal::disable_raw_mode();
    let _ = execute!(std::io::stdout(), DisableBracketedPaste, terminal::LeaveAlternateScreen, cursor::SetCursorStyle::DefaultUserShape, cursor::Show);
}

fn enter() -> Result<()> {
    terminal::enable_raw_mode()?;
    execute!(std::io::stdout(), terminal::EnterAlternateScreen, EnableBracketedPaste, cursor::SetCursorStyle::SteadyBlock)?;
    Ok(())
}

struct Guard;
impl Drop for Guard {
    fn drop(&mut self) {
        restore();
    }
}

/// Run `cadre <words>` outside the TUI with this session's global options, then wait for
/// a key. Ctrl-C goes to the command (this process ignores it meanwhile).
fn run_outside(app: &App, words: &[String]) -> Result<()> {
    restore();
    let mut out = std::io::stdout();
    let shown = words.join(" ");
    writeln!(out, "$ cadre {shown}")?;
    let exe = std::env::current_exe()?;
    let mut cmd = std::process::Command::new(exe);
    cmd.args(&app.forward).args(words);
    let status = match cmd.spawn() {
        Ok(mut child) => {
            #[cfg(unix)]
            let old = unsafe { libc::signal(libc::SIGINT, libc::SIG_IGN) };
            let st = child.wait();
            #[cfg(unix)]
            unsafe {
                libc::signal(libc::SIGINT, old);
            }
            st.map(|s| s.code().map(|c| c.to_string()).unwrap_or_else(|| "signal".into())).unwrap_or_else(|e| e.to_string())
        }
        Err(e) => format!("could not start: {e}"),
    };
    write!(out, "\n[exit {status}] press any key to return to cadre ")?;
    out.flush()?;
    terminal::enable_raw_mode()?;
    loop {
        if let Event::Key(k) = event::read()? {
            if k.kind != KeyEventKind::Release {
                break;
            }
        }
    }
    enter()?;
    Ok(())
}

/// The entry screen; returns the exit code.
pub fn run(l: Launch) -> Result<i32> {
    let ctx = crate::ctx::Ctx::new(l.opts.clone())?;
    let mut app = App::new(l, &ctx);
    drop(ctx);
    app.load();
    let prev = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        restore();
        prev(info);
    }));
    enter()?;
    let _g = Guard;
    let mut term = Terminal::new(CrosstermBackend::new(std::io::stdout()))?;
    term.clear()?;
    let frame = Duration::from_millis(1000 / app.robot.fps.max(1) as u64);
    let mut last = Instant::now() - frame;
    let mut dirty = true;
    while !app.quit {
        let size = term.size()?;
        let anim = app.animating(size.width, size.height) || app.busy.is_some() || app.quoting;
        if dirty || (anim && last.elapsed() >= frame) {
            term.draw(|f| draw(f, &app))?;
            last = Instant::now();
            dirty = false;
        }
        let wait = if anim { frame.saturating_sub(last.elapsed()) } else { Duration::from_millis(500) };
        if event::poll(wait)? {
            loop {
                match event::read()? {
                    Event::Key(k) => app.key(k),
                    Event::Paste(s) => app.paste(&s),
                    Event::Resize(..) => {}
                    _ => {}
                }
                dirty = true;
                if app.quit || !event::poll(Duration::ZERO)? {
                    break;
                }
            }
        }
        while let Ok(m) = app.rx.try_recv() {
            app.apply(m);
            dirty = true;
        }
        if let Some(words) = app.run.take() {
            run_outside(&app, &words)?;
            term.clear()?;
            dirty = true;
        }
    }
    Ok(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn opens_only_for_cadre_alone_on_a_terminal() {
        assert!(should_open("cadre", false, false, true, true, Some("xterm-256color")));
        // every other case keeps the old behavior
        assert!(!should_open("pearl", false, false, true, true, Some("xterm")));
        assert!(!should_open("cadre", true, false, true, true, Some("xterm")));
        assert!(!should_open("cadre", false, true, true, true, Some("xterm")));
        assert!(!should_open("cadre", false, false, false, true, Some("xterm")));
        assert!(!should_open("cadre", false, false, true, false, Some("xterm")));
        assert!(!should_open("cadre", false, false, true, true, Some("dumb")));
    }

    #[test]
    fn layout_at_reference_sizes() {
        // 120 x 40: the full robot, the wide wordmark, a 75-cell box, the bottom bar
        let p = plan_home(120, 40);
        assert_eq!((p.robot, p.logo, p.box_w, p.box_x), (RobotSize::Full, LogoKind::Wide, 75, 22));
        assert_eq!(p.footer_y, Some(38));
        assert_eq!(p.logo_y, p.robot_y + 25);
        assert_eq!(p.box_y, p.logo_y + 5);
        assert!(p.hints_y < 37);
        // 80 x 30: too short for the full robot, the half one fits
        let p = plan_home(80, 30);
        assert_eq!((p.robot, p.logo), (RobotSize::Half, LogoKind::Wide));
        assert!(p.hints_y < 28);
        // narrow: under 44 columns no full robot; the wordmark stacks
        let p = plan_home(40, 40);
        assert_eq!((p.robot, p.logo), (RobotSize::Half, LogoKind::Stacked));
        // tiny: no robot, words only, the prompt still fits
        let p = plan_home(28, 14);
        assert_eq!((p.robot, p.logo), (RobotSize::None, LogoKind::Text));
        assert!(p.hints_y < 14);
        // a pty that reports 0 x 0 (or any tiny size) never panics
        for w in 0..20 {
            for h in 0..20 {
                let _ = plan_home(w, h);
                let _ = plan_chat(w, h);
            }
        }
        // everything stays inside the screen at every size
        for w in 20..200 {
            for h in 12..80 {
                let p = plan_home(w, h);
                assert!(p.box_x + p.box_w <= w && p.hints_y < h, "{w}x{h}");
                if let Some(f) = p.footer_y {
                    assert!(p.hints_y < f, "{w}x{h}");
                }
                if p.robot == RobotSize::Full {
                    assert!(w >= 44 && h >= 39, "{w}x{h}");
                }
            }
        }
    }

    #[test]
    fn words_and_wrapping() {
        assert_eq!(split_words(r#"tx  abc "two words" ''"#), vec!["tx", "abc", "two words", ""]);
        assert_eq!(split_words("status"), vec!["status"]);
        assert_eq!(wrap("aaa bbb ccc", 7), vec!["aaa bbb", "ccc"]);
        assert_eq!(wrap("abcdefghij", 4), vec!["abcd", "efgh", "ij"]);
        assert_eq!(wrap("a\nb", 10), vec!["a", "b"]);
    }

    #[test]
    fn git_branch_of_a_checkout_and_a_worktree() {
        let d = std::env::temp_dir().join(format!("cadre-entry-git-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(d.join("repo/.git")).unwrap();
        std::fs::create_dir_all(d.join("repo/sub")).unwrap();
        std::fs::write(d.join("repo/.git/HEAD"), "ref: refs/heads/main\n").unwrap();
        assert_eq!(git_branch(&d.join("repo/sub")).as_deref(), Some("main"));
        std::fs::create_dir_all(d.join("wtgit")).unwrap();
        std::fs::write(d.join("wtgit/HEAD"), "ref: refs/heads/cli-entry\n").unwrap();
        std::fs::create_dir_all(d.join("wt")).unwrap();
        std::fs::write(d.join("wt/.git"), format!("gitdir: {}\n", d.join("wtgit").display())).unwrap();
        assert_eq!(git_branch(&d.join("wt")).as_deref(), Some("cli-entry"));
        let _ = std::fs::remove_dir_all(&d);
    }

    /// The whole screen renders into a buffer at the reference sizes without panics, and
    /// the wordmark, prompt and hints are where the plan says.
    #[test]
    fn renders_home_screen_into_a_buffer() {
        use ratatui::backend::TestBackend;
        for (w, h) in [(120u16, 40u16), (80, 30), (40, 20), (24, 12), (10, 5), (1, 1)] {
            let mut t = Terminal::new(TestBackend::new(w, h)).unwrap();
            let app = test_app();
            t.draw(|f| draw(f, &app)).unwrap();
            let b = t.backend().buffer().clone();
            let row = |y: u16| (0..w).map(|x| b[(x, y)].symbol().to_string()).collect::<String>();
            let p = plan_home(w, h);
            if p.box_y + BOX_ROWS <= h {
                assert!(row(p.box_y + 1).contains("Ask anything"), "{w}x{h}: {}", row(p.box_y + 1));
                assert!(row(p.box_y + 3).contains("Ask"), "{w}x{h}");
                assert!(row(p.hints_y).contains("ctrl+p commands") || w < 40, "{w}x{h}: {}", row(p.hints_y));
            }
            if p.logo == LogoKind::Wide {
                assert!(row(p.logo_y + 1).contains("█▀▀▀ ▀▀▀█ █▀▀█ █▀▀▀ █▀▀█ ▀▀▀█ █▀▀▀ █▀▀█ █▀▀▄ █▀▀▀ █▀▀▀"), "{}", row(p.logo_y + 1));
            }
            if let Some(fy) = p.footer_y {
                assert!(row(fy).contains(env!("CARGO_PKG_VERSION")) || w < 30, "{w}x{h}: {}", row(fy));
            }
        }
    }

    pub(crate) fn test_app() -> App {
        let (tx, rx) = channel();
        let robot = Robot::embedded();
        App {
            net: Net { api: None, gateway: None, key: None, api_key: None },
            opts: <crate::ctx::Opts as clap::FromArgMatches>::from_arg_matches(&<crate::ctx::Opts as clap::Args>::augment_args(clap::Command::new("t")).get_matches_from(["t"])).unwrap(),
            forward: vec![],
            commands: vec![PaletteCmd { name: "status".into(), about: "Health of everything".into(), needs_args: false }, PaletteCmd { name: "tx".into(), about: "A tx".into(), needs_args: true }],
            network: "devnet",
            me: None,
            color: Mode::TrueColor,
            robot_half: robot.half(),
            robot,
            start: Instant::now(),
            cwd: "~/pearl-sol:main".into(),
            mode: 0,
            agents: vec![AgentEntry { account: "A".into(), handle: "tide".into(), name: "Tide Clerk".into(), house: false }],
            agents_err: None,
            model: Some(Model { id: "llama-3.2-1b-instruct-w7a7".into(), server: "S".into() }),
            model_err: None,
            dm_live: Some(false),
            input: vec![],
            cur: 0,
            transcript: vec![],
            scroll: 0,
            history: vec![],
            busy: None,
            next_id: 1,
            quote: None,
            quoting: false,
            palette: None,
            tx,
            rx,
            quit: false,
            run: None,
        }
    }

    #[test]
    fn keys_tab_palette_and_quit() {
        let k = |c: KeyCode, m: KeyModifiers| KeyEvent::new(c, m);
        let mut a = test_app();
        // tab: Ask -> @tide (and back with shift+tab)
        a.key(k(KeyCode::Tab, KeyModifiers::NONE));
        assert_eq!(a.label(), "@tide");
        a.key(k(KeyCode::BackTab, KeyModifiers::SHIFT));
        assert_eq!(a.label(), "Ask");
        // ctrl+p opens the palette; typing filters; enter on a command with args fills the prompt
        a.key(k(KeyCode::Char('p'), KeyModifiers::CONTROL));
        assert!(a.palette.is_some());
        a.key(k(KeyCode::Char('t'), KeyModifiers::NONE));
        a.key(k(KeyCode::Char('x'), KeyModifiers::NONE));
        a.key(k(KeyCode::Enter, KeyModifiers::NONE));
        assert!(a.palette.is_none());
        assert_eq!(a.input.iter().collect::<String>(), "/tx ");
        // a command without args runs at once
        a.input.clear();
        a.cur = 0;
        a.key(k(KeyCode::Char('p'), KeyModifiers::CONTROL));
        a.key(k(KeyCode::Enter, KeyModifiers::NONE));
        assert_eq!(a.run.take(), Some(vec!["status".to_string()]));
        // a / line runs a command
        for c in "/block 12".chars() {
            a.key(k(KeyCode::Char(c), KeyModifiers::NONE));
        }
        a.key(k(KeyCode::Enter, KeyModifiers::NONE));
        assert_eq!(a.run.take(), Some(vec!["block".to_string(), "12".to_string()]));
        // asking with no wallet and no API key: the setup line, never an answer
        for c in "hi".chars() {
            a.key(k(KeyCode::Char(c), KeyModifiers::NONE));
        }
        a.key(k(KeyCode::Enter, KeyModifiers::NONE));
        assert!(a.busy.is_none());
        assert!(a.transcript.last().unwrap().text.contains("--key"));
        // a DM while DMs are not live: says so, sends nothing
        a.key(k(KeyCode::Tab, KeyModifiers::NONE));
        for c in "gm".chars() {
            a.key(k(KeyCode::Char(c), KeyModifiers::NONE));
        }
        a.key(k(KeyCode::Enter, KeyModifiers::NONE));
        assert!(a.transcript.last().unwrap().text.contains("not live"));
        // esc closes nothing open -> quits; ctrl+c always quits
        a.key(k(KeyCode::Esc, KeyModifiers::NONE));
        assert!(a.quit);
        let mut b = test_app();
        b.key(k(KeyCode::Char('c'), KeyModifiers::CONTROL));
        assert!(b.quit);
    }
}
