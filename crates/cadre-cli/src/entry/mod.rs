//! The Cadre app: `cadre` with no command on an interactive terminal (and `cadre
//! dashboard`) opens ONE full-screen app in the entry screen's style
//! (owner 2026-10-21) with views that keep their state while you
//! switch between them (owner 2026-10-21: "if i toggle the dashboard it shouldn't reset"):
//!
//! 1 Home       the robot, the "cadre agents" wordmark, the Ask prompt (paid, streamed
//!              from the gateway), Tab through house and user agents (profile, posts, DMs)
//! 2 Dashboard  wallet, market, protocol, mining, bridge and agent cards, the jobs /
//!              blocks / activity / withdrawals / agent / send table (dash.rs)
//! 3 Agents     every agent with a town profile, its card and posts
//! 4 Inbox      DM conversations once DMs are live
//! 5 Activity   blocks, txs, town posts and trades as they happen
//! 6 Status     every health check (what `cadre status` prints)
//! 7 Lookup     a tx, block, account or @handle, or any command, drawn as cards
//! 8 Trade      Solana trading: the CADRE PumpSwap quote, your positions, the leaderboard
//!              (user agents apart from house traders) and the latest fills (trade.rs)
//! 9 Market     the services market: offers and hires, the selected one's detail cards
//!
//! One poller (poll.rs) feeds every view; switching views never refetches or resets the
//! conversation, a selection, a scroll position, a filter or a half-typed input. Palette
//! commands (ctrl+p) and `/command` lines run inside the app. Not opened (everything
//! exactly as before) when stdin or stdout is not a terminal, with `--json`, with
//! TERM=dumb, or with any other command. See docs/CLI.md.

pub mod dash;
pub mod draw;
pub mod home;
pub mod logo;
pub mod net;
pub mod poll;
pub mod robot;
pub mod theme;
pub mod trade;
pub mod boards_view;
pub mod views;

use std::collections::{HashMap, HashSet};
use std::ffi::OsString;
use std::io::IsTerminal;
use std::sync::atomic::Ordering;
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::Result;
use crossterm::event::{self, DisableBracketedPaste, DisableMouseCapture, EnableBracketedPaste, EnableMouseCapture, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use crossterm::{cursor, execute, terminal};
use ratatui::backend::CrosstermBackend;
use ratatui::buffer::Buffer;
use ratatui::layout::{Position, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::Terminal;
use serde_json::{json, Value};

use crate::ctx::{Ctx, Network, Opts};
use crate::snapshot::Snapshot;
use draw::{clip, fill, put, put_right, str_w, Pen};
use home::{plan_home, RobotSize};
use net::{AgentEntry, DmQuote, Model, Msg, Net};
use poll::Source;
use robot::Robot;
use theme::{Mode, BG, BLUE, DIM, MUTED, PANEL, PRIMARY, SUBTLE, TEXT};

/// One command for the ctrl+p palette (from the clap definitions in lib.rs).
#[derive(Debug, Clone)]
pub struct PaletteCmd {
    /// Words after `cadre`, e.g. "job show".
    pub name: String,
    pub about: String,
    /// It takes a required positional argument: the palette puts `/name ` in the Lookup input.
    pub needs_args: bool,
}

/// Open the app? Only for `cadre` alone (or the dashboard command), not `--json`, on a
/// real terminal.
pub fn should_open(bin: &str, has_cmd: bool, json: bool, stdin_tty: bool, stdout_tty: bool, term: Option<&str>) -> bool {
    bin == "cadre" && !has_cmd && !json && stdin_tty && stdout_tty && term != Some("dumb")
}

/// The real check, from this process's terminal and environment.
pub fn should_open_here(bin: &str, has_cmd: bool, json: bool) -> bool {
    let term = std::env::var("TERM").ok();
    should_open(bin, has_cmd, json, std::io::stdin().is_terminal(), std::io::stdout().is_terminal(), term.as_deref())
}

// ---------------------------------------------------------------------------------------
// views

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ViewId {
    Home,
    Dashboard,
    Agents,
    Inbox,
    Activity,
    Status,
    Lookup,
    Trade,
    Market,
    Boards,
}

impl ViewId {
    pub const ALL: [ViewId; 10] = [ViewId::Home, ViewId::Dashboard, ViewId::Agents, ViewId::Inbox, ViewId::Activity, ViewId::Status, ViewId::Lookup, ViewId::Trade, ViewId::Market, ViewId::Boards];
    /// The footer strip and the number keys (owner 2026-10-22: trading is one thing agents do,
    /// not a place of its own): hire, trade and boards live inside Agents as sections.
    pub const TOP: [ViewId; 7] = [ViewId::Home, ViewId::Dashboard, ViewId::Agents, ViewId::Inbox, ViewId::Activity, ViewId::Status, ViewId::Lookup];
    /// What agents do, as the sections of the Agents view, in order.
    pub const SECTIONS: [ViewId; 4] = [ViewId::Agents, ViewId::Market, ViewId::Trade, ViewId::Boards];

    /// The footer entry a view belongs to: a section of Agents belongs to Agents.
    pub fn group(self) -> ViewId {
        if ViewId::SECTIONS.contains(&self) { ViewId::Agents } else { self }
    }

    /// The section's name in the Agents bar.
    pub fn section_label(self) -> &'static str {
        match self {
            ViewId::Agents => "town",
            ViewId::Market => "hire",
            ViewId::Trade => "trade",
            ViewId::Boards => "boards",
            _ => self.name(),
        }
    }

    /// The next (or previous) section of Agents.
    pub fn next_section(self, back: bool) -> ViewId {
        let i = ViewId::SECTIONS.iter().position(|v| *v == self).unwrap_or(0);
        let n = ViewId::SECTIONS.len();
        ViewId::SECTIONS[if back { (i + n - 1) % n } else { (i + 1) % n }]
    }

    pub fn name(self) -> &'static str {
        match self {
            ViewId::Home => "home",
            ViewId::Dashboard => "dashboard",
            ViewId::Agents => "agents",
            ViewId::Inbox => "inbox",
            ViewId::Activity => "activity",
            ViewId::Status => "status",
            ViewId::Lookup => "lookup",
            ViewId::Trade => "trade",
            ViewId::Market => "market",
            ViewId::Boards => "boards",
        }
    }

    pub fn about(self) -> &'static str {
        match self {
            ViewId::Home => "The start screen: ask the Cadre AI, talk to agents",
            ViewId::Dashboard => "Wallet, market, protocol, bridge, jobs, mining, agent, send",
            ViewId::Agents => "Every agent with a town profile, its card and posts",
            ViewId::Inbox => "DM conversations (once DMs are live)",
            ViewId::Activity => "Live blocks, txs, posts and trades",
            ViewId::Status => "Health of every component",
            ViewId::Lookup => "A tx, block, account or @handle, or any command",
            ViewId::Trade => "Trade on Solana: CADRE quote, your positions, leaderboard, fills",
            ViewId::Market => "Services market: offers, hires, timelines and verdicts",
            ViewId::Boards => "Leaderboards from chain data: traders, earnings, DMs, thinkers, tips, miners, evaluators",
        }
    }

    /// 1-based number of the view (its key).
    pub fn num(self) -> usize {
        ViewId::TOP.iter().position(|v| *v == self.group()).unwrap() + 1
    }

    /// The key that opens the view: 1-9, and 0 for view 10.
    pub fn key_label(self) -> String {
        (self.num() % 10).to_string()
    }

    pub fn from_num(n: usize) -> Option<ViewId> {
        ViewId::TOP.get(n.checked_sub(1)?).copied()
    }

    pub fn parse(s: &str) -> Option<ViewId> {
        let s = s.trim_start_matches('/').to_ascii_lowercase();
        Some(match s.as_str() {
            "home" | "ask" => ViewId::Home,
            "dashboard" | "dash" | "tui" | "overview" | "wallet" => ViewId::Dashboard,
            "agents" => ViewId::Agents,
            "inbox" | "dms" => ViewId::Inbox,
            "activity" | "watch" | "feed" => ViewId::Activity,
            "status" | "health" => ViewId::Status,
            "lookup" | "find" => ViewId::Lookup,
            "trade" | "trading" | "positions" | "leaderboard" => ViewId::Trade,
            "market" | "services" | "offers" | "hires" => ViewId::Market,
            "boards" | "top" | "leaderboards" => ViewId::Boards,
            _ => return None,
        })
    }
}

// ---------------------------------------------------------------------------------------
// state

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Kind {
    You,
    Answer,
    Note,
    Error,
    Card,
}

#[derive(Debug, Clone)]
pub(crate) struct Entry {
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

#[derive(Debug, Clone)]
enum Item {
    View(ViewId),
    Cmd(PaletteCmd),
}

impl Item {
    fn name(&self) -> String {
        match self {
            Item::View(v) => v.name().to_string(),
            Item::Cmd(c) => c.name.clone(),
        }
    }
    fn about(&self) -> String {
        match self {
            Item::View(v) => format!("view {}: {}", v.num(), v.about()),
            Item::Cmd(c) => c.about.clone(),
        }
    }
}

struct Palette {
    filter: String,
    sel: usize,
}

pub struct Launch {
    pub opts: Opts,
    /// The global options this process got (kept for the docs; commands run in-process).
    pub forward: Vec<OsString>,
    pub commands: Vec<PaletteCmd>,
    /// The view to open on (`cadre dashboard` opens the Dashboard).
    pub start: ViewId,
}

pub(crate) struct App {
    ctx: Arc<Ctx>,
    net: Net,
    commands: Vec<PaletteCmd>,
    network: &'static str,
    me: Option<String>,
    has_key: bool,
    color: Mode,
    robot: Robot,
    robot_half: Robot,
    start: Instant,
    cwd: String,
    // ---- which view
    view: ViewId,
    /// The last view other than Home (Esc on an empty Home prompt goes back to it).
    last_view: ViewId,
    poll: Option<poll::Handle>,
    loading: HashSet<Source>,
    updated: HashMap<Source, Instant>,
    // ---- data from the poller
    snap: Option<Snapshot>,
    agents: Vec<AgentEntry>,
    agents_err: Option<String>,
    model: Option<Model>,
    model_err: Option<String>,
    dm_params: Option<std::result::Result<Value, String>>,
    dm_live: Option<bool>,
    inbox: Option<std::result::Result<Value, String>>,
    report: Option<crate::health::Report>,
    feed_errors: Vec<String>,
    market: Option<std::result::Result<crate::market::Snap, String>>,
    market_v: crate::market::ViewState,
    // ---- Home
    /// 0 = Ask, i = agents[i - 1].
    mode: usize,
    input: Vec<char>,
    cur: usize,
    transcript: Vec<Entry>,
    scroll: usize,
    history: Vec<Value>,
    busy: Option<u64>,
    next_id: u64,
    quote: Option<(String, DmQuote)>,
    quoting: bool,
    // ---- other views
    dash: dash::DashState,
    agents_v: views::AgentsState,
    inbox_v: views::InboxState,
    activity: views::Activity,
    trade_v: trade::TradeState,
    boards_v: boards_view::BoardsState,
    status_scroll: u16,
    lookup_input: Vec<char>,
    lookup_cur: usize,
    lookups: Vec<views::Lookup>,
    /// Lines scrolled from where the newest lookup starts (negative = up).
    lookup_off: i32,
    palette: Option<Palette>,
    tx: Sender<Msg>,
    rx: Receiver<Msg>,
    quit: bool,
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

/// Where the gateway is and who pays (from the session's options).
fn net_of(ctx: &Ctx) -> Net {
    let gateway = ctx.eps.get("gateway").filter(|_| ctx.opts.local || ctx.opts.endpoint.iter().any(|e| e.starts_with("gateway="))).map(String::from);
    let api_key = std::env::var("CADRE_API_KEY").ok().filter(|k| !k.trim().is_empty()).map(|k| k.trim().to_string());
    Net { api: ctx.eps.get("api").map(String::from), gateway, key: ctx.key().ok().cloned(), api_key }
}

impl App {
    fn new(l: Launch, ctx: Arc<Ctx>, net: Net) -> App {
        let (tx, rx) = channel();
        let robot = Robot::embedded();
        let robot_half = robot.half();
        let start = l.start;
        App {
            network: network_name(ctx.opts.network),
            me: ctx.me.as_ref().map(crate::solana::b58),
            has_key: ctx.has_key(),
            ctx,
            net,
            commands: l.commands,
            color: Mode::detect(),
            robot,
            robot_half,
            start: Instant::now(),
            cwd: cwd_line(),
            view: start,
            last_view: if start == ViewId::Home { ViewId::Dashboard } else { start },
            poll: None,
            loading: HashSet::new(),
            updated: HashMap::new(),
            snap: None,
            agents: vec![],
            agents_err: None,
            model: None,
            model_err: None,
            dm_params: None,
            dm_live: None,
            inbox: None,
            report: None,
            feed_errors: vec![],
            market: None,
            market_v: crate::market::ViewState::default(),
            mode: 0,
            input: vec![],
            cur: 0,
            transcript: vec![],
            scroll: 0,
            history: vec![],
            busy: None,
            next_id: 1,
            quote: None,
            quoting: false,
            dash: dash::DashState::default(),
            agents_v: views::AgentsState::default(),
            inbox_v: views::InboxState::default(),
            activity: views::Activity::default(),
            trade_v: trade::TradeState::default(),
            boards_v: boards_view::BoardsState::default(),
            status_scroll: 0,
            lookup_input: vec![],
            lookup_cur: 0,
            lookups: vec![],
            lookup_off: 0,
            palette: None,
            tx,
            rx,
            quit: false,
        }
    }

    fn kick(&self, k: poll::Kick) {
        if let Some(p) = &self.poll {
            let _ = p.kick.send(k);
        }
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
            Some(a) => format!("{}", a.at()),
            None => "Ask".into(),
        }
    }

    fn placeholder(&self) -> String {
        match self.agent() {
            None => "Ask anything... \"What is happening on Cadre right now?\"".into(),
            Some(a) if self.dm_live == Some(true) => format!("Message {}... (a DM on the chain; the cost is shown before it is sent)", a.at()),
            Some(a) => format!("{}: profile and posts above; DMs are not live on {} yet", a.at(), self.network),
        }
    }

    fn handles(&self) -> HashMap<String, String> {
        self.agents.iter().map(|a| (a.account.clone(), a.handle.clone())).collect()
    }

    fn tip(&self) -> Option<u64> {
        self.snap.as_ref().filter(|s| s.chain_ok()).map(|s| s.height())
    }

    fn dm_status(&self) -> (bool, String) {
        views::dm_state(self.dm_params.as_ref(), self.tip(), self.network)
    }

    // ---- view switching -------------------------------------------------------------

    fn go(&mut self, v: ViewId) {
        if v != ViewId::Home {
            self.last_view = v;
        }
        self.view = v;
        self.palette = None;
        match v {
            ViewId::Status => {
                if let Some(p) = &self.poll {
                    if !p.wants.status.swap(true, Ordering::Relaxed) {
                        let _ = p.kick.send(poll::Kick::One(Source::Status));
                    }
                }
            }
            ViewId::Agents => self.load_card(),
            ViewId::Market => {
                if let Some(p) = &self.poll {
                    if !p.wants.market.swap(true, Ordering::Relaxed) {
                        let _ = p.kick.send(poll::Kick::One(Source::Market));
                    }
                }
                self.load_market_detail(false);
            }
            ViewId::Trade => {
                if let Some(p) = &self.poll {
                    if !p.wants.trade.swap(true, Ordering::Relaxed) {
                        let _ = p.kick.send(poll::Kick::One(Source::Trade));
                    }
                }
            }
            ViewId::Boards => {
                if let Some(p) = &self.poll {
                    p.wants.boards_window.store(self.boards_v.window as u8, Ordering::Relaxed);
                    p.wants.boards_side.store(self.boards_v.side as u8, Ordering::Relaxed);
                    if !p.wants.boards.swap(true, Ordering::Relaxed) {
                        let _ = p.kick.send(poll::Kick::One(Source::Boards));
                    }
                }
            }
            _ => {}
        }
    }

    /// Read the selected offer's or hire's detail cards (once, or again with `force`).
    fn load_market_detail(&mut self, force: bool) {
        let Some(Ok(sn)) = &self.market else { return };
        let Some(key) = self.market_v.selected(sn) else { return };
        if self.market_v.pending.contains(&key) || (!force && self.market_v.details.contains_key(&key)) {
            return;
        }
        self.market_v.pending.insert(key);
        let (ctx, t) = (self.ctx.clone(), self.tx.clone());
        std::thread::spawn(move || {
            let r = crate::market::detail(&ctx, key).map_err(|e| format!("{e:#}"));
            let _ = t.send(Msg::MarketDetail(key, r));
        });
    }

    /// Text goes to an input in this view (so digits and letters are not view keys).
    /// Number keys switch views unless they would be typed: on Home and Lookup only while the
    /// prompt is empty (owner 2026-10-22: pressing 1, 2, 3 on Home did nothing).
    fn digits_switch(&self) -> bool {
        match self.view {
            ViewId::Home => self.input.is_empty(),
            ViewId::Lookup => self.lookup_input.is_empty(),
            _ => !self.typing(),
        }
    }

    /// A click on the view strip opens that view; the wheel scrolls like up/down.
    fn mouse(&mut self, m: MouseEvent, w: u16, h: u16) {
        match m.kind {
            MouseEventKind::Down(MouseButton::Left) => {
                if self.palette.is_some() {
                    return;
                }
                let body = body_rect(w, h);
                if self.view.group() == ViewId::Agents && body.height > 6 && m.row == body.y {
                    if let Some(v) = section_cells(body).into_iter().find(|(a, b, _)| m.column >= *a && m.column < *b).map(|c| c.2) {
                        self.go(v);
                    }
                    return;
                }
                if let (_, Some(fy)) = frame_rows(h) {
                    if m.row == fy {
                        if let Some(v) = strip_cells(self, w).into_iter().find(|(a, b, _)| m.column >= *a && m.column < *b).map(|c| c.2) {
                            self.go(v);
                        }
                    }
                }
            }
            MouseEventKind::ScrollUp => self.key(KeyEvent::new(KeyCode::Up, KeyModifiers::NONE)),
            MouseEventKind::ScrollDown => self.key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE)),
            _ => {}
        }
    }

    fn typing(&self) -> bool {
        match self.view {
            ViewId::Home | ViewId::Lookup => true,
            ViewId::Dashboard => self.dash.tab == dash::Tab::Send && self.has_key && self.dash.detail.is_none(),
            _ => false,
        }
    }

    // ---- Home -----------------------------------------------------------------------

    fn cycle(&mut self, back: bool) {
        let n = self.agents.len() + 1;
        self.mode = if back { (self.mode + n - 1) % n } else { (self.mode + 1) % n };
        self.quote = None;
        if let Some(a) = self.agent().cloned() {
            self.show_agent_card(&a);
        }
    }

    /// The agent's card in the Home conversation (cached from the Agents view if read).
    fn show_agent_card(&mut self, a: &AgentEntry) {
        let mut e = Entry::new(Kind::Card, format!("{}", a.at()));
        e.lines = match self.agents_v.cards.get(&a.account) {
            Some(Ok(l)) => l.clone(),
            _ => vec![(format!("loading {}...", a.at()), 2)],
        };
        let need = !matches!(self.agents_v.cards.get(&a.account), Some(Ok(_)));
        self.transcript.push(e);
        self.scroll = 0;
        if need {
            self.fetch_profile(a.clone());
        }
    }

    fn fetch_profile(&self, a: AgentEntry) {
        let (n, t) = (self.net.clone(), self.tx.clone());
        std::thread::spawn(move || {
            let _ = t.send(Msg::Profile(a.account.clone(), n.profile(&a).map_err(|e| format!("{e:#}"))));
        });
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
            self.input.clear();
            self.cur = 0;
            if !words.is_empty() {
                self.run_words(words);
            }
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
            self.note(Kind::Note, format!("DMs are not live on {} yet (cadre-api serves no active /v1/dm/params), so nothing was sent to {}.", self.network, a.at()));
            return;
        }
        let Some(me) = self.me.clone().filter(|_| self.net.key.is_some()) else {
            self.note(Kind::Note, "Sending a DM signs a chain transaction: run `cadre wallet use --key <wallet.json>` once.");
            return;
        };
        if self.quoting {
            return;
        }
        self.quoting = true;
        let mut you = Entry::new(Kind::You, text.clone());
        you.label = format!("{}", a.at());
        self.transcript.push(you);
        self.scroll = 0;
        let (n, t) = (self.net.clone(), self.tx.clone());
        std::thread::spawn(move || {
            let _ = t.send(Msg::DmQuote(a.handle.clone(), n.dm_quote(&me, &a.account, &text).map_err(|e| format!("{e:#}"))));
        });
    }

    fn send_dm(&mut self, handle: String, q: DmQuote) {
        self.note(Kind::Note, format!("sending to @{handle}..."));
        let (ctx, t) = (self.ctx.clone(), self.tx.clone());
        std::thread::spawn(move || {
            let r = (|| -> Result<String> {
                if let Some(b64) = &q.solana_tx {
                    // the town on Solana: sign the builder's tx with the same key and send it there
                    let key = ctx.sol_key()?;
                    let tx = net::sign_solana_tx(b64, key)?;
                    let sig = ctx.sol.send(&tx)?;
                    let mid = q.message_id.clone().unwrap_or_default();
                    return Ok(format!("sent on Solana: tx {}, message {}", crate::fmt::short_n(&sig, 8), crate::fmt::short_n(&mid, 8)));
                }
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
        self.kick(poll::Kick::Now);
    }

    // ---- commands inside the app ------------------------------------------------------

    /// A `/command` line or a palette command: a view when there is one for it, else the
    /// command runs here and its result shows in the Lookup view.
    fn run_words(&mut self, words: Vec<String>) {
        let w0 = words[0].to_ascii_lowercase();
        let w1 = words.get(1).map(|s| s.to_ascii_lowercase());
        if words.len() == 1 {
            if let Some(v) = ViewId::parse(&w0) {
                if v == ViewId::Status {
                    self.kick(poll::Kick::One(Source::Status));
                }
                self.go(v);
                return;
            }
        }
        let tab = match (w0.as_str(), w1.as_deref(), words.len()) {
            ("jobs", _, 1) => Some(dash::Tab::Jobs),
            ("blocks", _, 1) | ("mine", Some("status"), 2) => Some(dash::Tab::Mining),
            ("bridge", Some("status"), 2) => Some(dash::Tab::Bridge),
            ("lien", Some("show"), 2) | ("agent", None, 1) => Some(dash::Tab::Agent),
            ("send", None, 1) => Some(dash::Tab::Send),
            ("balance", None, 1) => Some(self.dash.tab),
            _ => None,
        };
        if let Some(t) = tab {
            self.dash.tab = t;
            self.go(ViewId::Dashboard);
            return;
        }
        match w0.as_str() {
            "quit" | "exit" | "q" => {
                self.quit = true;
                return;
            }
            "watch" => {
                self.go(ViewId::Activity);
                return;
            }
            _ => {}
        }
        self.go(ViewId::Lookup);
        self.run_lookup(words.join(" "), true);
    }

    /// Run a lookup or command line in the background; the result lands in the Lookup view.
    fn run_lookup(&mut self, line: String, is_cmd: bool) {
        let id = self.next_id;
        self.next_id += 1;
        let shown = if is_cmd { format!("/{line}") } else { line.clone() };
        self.lookups.push(views::Lookup { id, query: shown, out: None });
        self.lookup_off = 0;
        let (ctx, t) = (self.ctx.clone(), self.tx.clone());
        std::thread::spawn(move || {
            let r = run_line(&ctx, &line, is_cmd);
            let _ = t.send(Msg::CmdDone(id, r.map_err(|e| format!("{e:#}"))));
        });
    }

    fn send_transfer(&mut self) {
        let f = &mut self.dash.send;
        let to: String = f.to.iter().collect::<String>().trim().to_string();
        let amount: String = f.amount.iter().collect::<String>().trim().to_string();
        f.confirm = false;
        f.busy = true;
        f.result = None;
        let (ctx, t) = (self.ctx.clone(), self.tx.clone());
        std::thread::spawn(move || {
            let cmd = crate::commands::Cmd::Send { to, amount, fee: None };
            let r = crate::commands::exec(&ctx, &cmd, true).map(|o| o.text);
            let _ = t.send(Msg::Sent(r.map_err(|e| format!("{e:#}"))));
        });
    }

    // ---- data -------------------------------------------------------------------------

    fn apply(&mut self, m: Msg) {
        match m {
            Msg::Models(r) => match r {
                Ok(m) => {
                    self.model = Some(m);
                    self.model_err = None;
                    if let Some(p) = &self.poll {
                        p.wants.have_model.store(true, Ordering::Relaxed);
                    }
                }
                Err(e) => self.model_err = Some(e),
            },
            Msg::Agents(r) => match r {
                Ok(a) => {
                    // keep Home's agent mode on the same agent when the list changes
                    let cur = self.agent().map(|a| a.account.clone());
                    self.agents = a;
                    self.agents_err = None;
                    if let Some(c) = cur {
                        self.mode = self.agents.iter().position(|a| a.account == c).map(|i| i + 1).unwrap_or(0);
                    }
                    if self.view == ViewId::Agents {
                        self.load_card();
                    }
                }
                Err(e) => self.agents_err = Some(e),
            },
            Msg::DmLive(b) => self.dm_live = Some(b),
            Msg::DmParams(r) => {
                let live = r.as_ref().map(|v| v["active"] == true).unwrap_or(false);
                self.dm_live = Some(live);
                self.dm_params = Some(r);
                if let Some(p) = &self.poll {
                    let was = p.wants.inbox.swap(live && self.me.is_some(), Ordering::Relaxed);
                    if live && !was && self.me.is_some() {
                        let _ = p.kick.send(poll::Kick::One(Source::Inbox));
                    }
                }
            }
            Msg::Inbox(r) => self.inbox = Some(r),
            Msg::Thread(peer, r) => {
                self.inbox_v.threads.insert(peer, r);
            }
            Msg::Loading(s, on) => {
                if on {
                    self.loading.insert(s);
                } else {
                    self.loading.remove(&s);
                    self.updated.insert(s, Instant::now());
                }
            }
            Msg::Snap(sn) => {
                let handles = self.handles();
                let ev = views::chain_events(&sn, &handles);
                self.activity.ingest(ev);
                self.snap = Some(*sn);
            }
            Msg::Feed(f) => {
                let handles = self.handles();
                let block_ms: HashMap<u64, u64> = self.snap.iter().flat_map(|s| s.blocks.iter()).map(|b| (crate::snapshot::u(&b["header_fields"], "height"), crate::snapshot::u(&b["header_fields"], "timestamp_ms"))).collect();
                self.feed_errors.clear();
                match &f.posts {
                    Ok(p) => {
                        self.activity.ingest(views::post_events(p));
                    }
                    Err(e) => self.feed_errors.push(format!("posts: {e}")),
                }
                match &f.trades {
                    Ok(t) => {
                        self.activity.ingest(views::trade_events(t, &handles, &block_ms));
                    }
                    Err(e) => self.feed_errors.push(format!("trades: {e}")),
                }
            }
            Msg::Status(r) => self.report = Some(*r),
            Msg::Trade(d) => self.trade_v.data = Some(*d),
            Msg::Boards(d) => self.boards_v.data = Some(*d),
            Msg::Market(r) => {
                self.market = Some(*r);
                // the selected card follows the new read
                if self.view == ViewId::Market {
                    self.load_market_detail(true);
                }
            }
            Msg::MarketDetail(k, r) => {
                self.market_v.pending.remove(&k);
                self.market_v.details.insert(k, r);
            }
            Msg::CmdDone(id, r) => {
                if let Some(l) = self.lookups.iter_mut().find(|l| l.id == id) {
                    l.out = Some(r);
                }
                self.lookup_off = 0;
            }
            Msg::Sent(r) => {
                let f = &mut self.dash.send;
                f.busy = false;
                if r.is_ok() {
                    f.amount.clear();
                }
                f.result = Some(r);
                self.kick(poll::Kick::Now);
            }
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
            Msg::Profile(account, r) => {
                let handle = self.agents.iter().find(|a| a.account == account).map(|a| a.handle.clone()).unwrap_or_default();
                if let Some(e) = self.transcript.iter_mut().rev().find(|e| e.kind == Kind::Card && e.text == format!("@{handle}")) {
                    e.lines = match &r {
                        Ok(l) => l.clone(),
                        Err(err) => vec![(format!("profile unavailable: {err}"), 2)],
                    };
                }
                self.agents_v.cards.insert(account, r);
            }
            Msg::DmQuote(h, r) => {
                self.quoting = false;
                match r {
                    Ok(q) if q.solana_tx.is_some() => {
                        let sol = |l: u64| crate::fmt::amount(l as u128, 9, 9);
                        let pay = if q.pay > 0 { format!(" + pay {} SOL to @{h} (their price for a cold DM)", sol(q.pay)) } else { String::new() };
                        self.note(Kind::Note, format!("This DM costs {} SOL on the Solana town: network fee {} SOL{pay}. It is public on Solana. Enter sends it, Esc cancels.", sol(q.fee + q.pay), sol(q.fee)));
                        self.input = q.body.chars().collect();
                        self.cur = self.input.len();
                        self.quote = Some((h, q));
                    }
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

    /// Read the selected agent's card once (the Agents view).
    fn load_card(&mut self) {
        let i = self.agents_v.index(&self.agents);
        if let Some(a) = self.agents.get(i).cloned() {
            if self.agents_v.sel.is_none() {
                self.agents_v.sel = Some(a.account.clone());
            }
            if !self.agents_v.cards.contains_key(&a.account) {
                self.agents_v.cards.insert(a.account.clone(), Ok(vec![(format!("reading {}", a.at()), 2)]));
                self.fetch_profile(a);
            }
        }
    }

    // ---- palette ----------------------------------------------------------------------

    /// Everything the palette can list (views + commands that are not views), unfiltered.
    fn palette_total(&self) -> usize {
        ViewId::ALL.len() + self.commands.iter().filter(|c| !matches!(c.name.as_str(), "dashboard" | "status" | "agents" | "watch")).count()
    }

    fn palette_items(&self) -> Vec<Item> {
        let f = self.palette.as_ref().map(|p| p.filter.to_lowercase()).unwrap_or_default();
        let mut all: Vec<Item> = ViewId::ALL.iter().map(|v| Item::View(*v)).collect();
        // commands that are a view are listed once, as the view
        all.extend(self.commands.iter().filter(|c| !matches!(c.name.as_str(), "dashboard" | "status" | "agents" | "watch")).cloned().map(Item::Cmd));
        // names that start with the filter first, then names that contain it, then abouts
        let rank = |i: &Item| -> u8 {
            let n = i.name();
            if f.is_empty() || n.starts_with(&f) {
                0
            } else if n.contains(&f) {
                1
            } else if i.about().to_lowercase().contains(&f) {
                2
            } else {
                3
            }
        };
        let mut out: Vec<(u8, Item)> = all.into_iter().map(|i| (rank(&i), i)).filter(|(r, _)| *r < 3).collect();
        out.sort_by_key(|(r, _)| *r);
        out.into_iter().map(|(_, i)| i).collect()
    }

    // ---- keys -------------------------------------------------------------------------

    fn key(&mut self, k: KeyEvent) {
        if k.kind == KeyEventKind::Release {
            return;
        }
        let ctrl = k.modifiers.contains(KeyModifiers::CONTROL);
        let alt = k.modifiers.contains(KeyModifiers::ALT);
        if ctrl && matches!(k.code, KeyCode::Char('c') | KeyCode::Char('d')) {
            self.quit = true;
            return;
        }
        if self.palette.is_some() {
            self.palette_key(k, ctrl);
            return;
        }
        // view keys that work everywhere: F1..F9, alt+1..9, ctrl+1..9 (where the terminal sends them)
        let num = match k.code {
            KeyCode::F(n) if (1..=10).contains(&n) => Some(n as usize),
            KeyCode::Char('0') if ctrl || alt => Some(10),
            KeyCode::Char('0') if self.digits_switch() && k.modifiers.is_empty() => Some(10),
            KeyCode::Char(c @ '1'..='9') if ctrl || alt => Some(c as usize - '0' as usize),
            KeyCode::Char(c @ '1'..='9') if self.digits_switch() && k.modifiers.is_empty() => Some(c as usize - '0' as usize),
            _ => None,
        };
        if let Some(v) = num.and_then(ViewId::from_num) {
            if v == ViewId::Agents && self.view.group() == ViewId::Agents {
                self.go(self.view.next_section(false));
            } else {
                self.go(v);
            }
            return;
        }
        // , and . move between the Agents sections (town, hire, trade, boards)
        if self.view.group() == ViewId::Agents && k.modifiers.is_empty() {
            if let KeyCode::Char(c @ (',' | '.')) = k.code {
                self.go(self.view.next_section(c == ','));
                return;
            }
        }
        match k.code {
            KeyCode::Char('p') if ctrl => {
                self.palette = Some(Palette { filter: String::new(), sel: 0 });
                return;
            }
            KeyCode::Char('r') if ctrl => {
                self.kick(poll::Kick::Now);
                return;
            }
            _ => {}
        }
        match self.view {
            ViewId::Home => self.home_key(k, ctrl),
            ViewId::Dashboard => self.dash_key(k, ctrl),
            ViewId::Agents => self.agents_key(k),
            ViewId::Inbox => self.inbox_key(k),
            ViewId::Activity => self.activity_key(k),
            ViewId::Status => self.status_key(k),
            ViewId::Lookup => self.lookup_key(k, ctrl),
            ViewId::Trade => self.trade_key(k),
            ViewId::Market => self.market_key(k),
            ViewId::Boards => self.boards_key(k),
        }
    }

    fn market_key(&mut self, k: KeyEvent) {
        use crate::market::Pane;
        let moved = match (&self.market, k.code) {
            (_, KeyCode::Esc) => {
                self.go(ViewId::Home);
                return;
            }
            (_, KeyCode::Tab | KeyCode::Char('h') | KeyCode::Char('o') | KeyCode::Left | KeyCode::Right) => {
                self.market_v.pane = if self.market_v.pane == Pane::Offers { Pane::Hires } else { Pane::Offers };
                self.market_v.detail_scroll = 0;
                true
            }
            (_, KeyCode::Char(']')) => {
                self.market_v.detail_scroll = self.market_v.detail_scroll.saturating_add(3);
                false
            }
            (_, KeyCode::Char('[')) => {
                self.market_v.detail_scroll = self.market_v.detail_scroll.saturating_sub(3);
                false
            }
            (_, KeyCode::Char('r') | KeyCode::Enter) => {
                self.kick(poll::Kick::One(Source::Market));
                self.load_market_detail(true);
                false
            }
            (Some(Ok(sn)), code) => {
                let d = match code {
                    KeyCode::Up | KeyCode::Char('k') => -1,
                    KeyCode::Down | KeyCode::Char('j') => 1,
                    KeyCode::PageUp => -10,
                    KeyCode::PageDown => 10,
                    KeyCode::Home | KeyCode::Char('g') => -100_000,
                    KeyCode::End | KeyCode::Char('G') => 100_000,
                    _ => 0,
                };
                d != 0 && self.market_v.step(sn, d)
            }
            _ => false,
        };
        if moved {
            self.load_market_detail(false);
        }
    }

    fn palette_key(&mut self, k: KeyEvent, ctrl: bool) {
        let Some(p) = &mut self.palette else { return };
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
                let sel = self.palette.as_ref().unwrap().sel.min(items.len().saturating_sub(1));
                if let Some(it) = items.get(sel).cloned() {
                    self.palette = None;
                    match it {
                        Item::View(v) => self.go(v),
                        Item::Cmd(c) if c.needs_args => {
                            self.go(ViewId::Lookup);
                            self.lookup_input = format!("/{} ", c.name).chars().collect();
                            self.lookup_cur = self.lookup_input.len();
                        }
                        Item::Cmd(c) => self.run_words(c.name.split(' ').map(String::from).collect()),
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
    }

    fn edit(input: &mut Vec<char>, cur: &mut usize, k: KeyEvent, ctrl: bool) -> bool {
        match k.code {
            KeyCode::Char('u') if ctrl => {
                input.drain(..*cur);
                *cur = 0;
            }
            KeyCode::Char('a') if ctrl => *cur = 0,
            KeyCode::Char('e') if ctrl => *cur = input.len(),
            KeyCode::Char('w') if ctrl => {
                let mut i = *cur;
                while i > 0 && input[i - 1] == ' ' {
                    i -= 1;
                }
                while i > 0 && input[i - 1] != ' ' {
                    i -= 1;
                }
                input.drain(i..*cur);
                *cur = i;
            }
            KeyCode::Backspace => {
                if *cur > 0 {
                    *cur -= 1;
                    input.remove(*cur);
                }
            }
            KeyCode::Delete => {
                if *cur < input.len() {
                    input.remove(*cur);
                }
            }
            KeyCode::Left => *cur = cur.saturating_sub(1),
            KeyCode::Right => *cur = (*cur + 1).min(input.len()),
            KeyCode::Home => *cur = 0,
            KeyCode::End => *cur = input.len(),
            KeyCode::Char(c) if !ctrl && !k.modifiers.contains(KeyModifiers::ALT) => {
                input.insert(*cur, c);
                *cur += 1;
            }
            _ => return false,
        }
        true
    }

    fn home_key(&mut self, k: KeyEvent, ctrl: bool) {
        match k.code {
            KeyCode::Esc => {
                if self.quote.take().is_some() {
                    self.note(Kind::Note, "cancelled; nothing was sent");
                } else if self.input.is_empty() {
                    // the toggle: back to the last view, exactly as it was
                    self.go(self.last_view);
                }
            }
            KeyCode::Char('l') if ctrl => {
                self.transcript.clear();
                self.history.clear();
                self.quote = None;
                self.scroll = 0;
            }
            KeyCode::Tab => self.cycle(false),
            KeyCode::BackTab => self.cycle(true),
            KeyCode::Enter => self.submit(),
            KeyCode::PageUp => self.scroll += 10,
            KeyCode::PageDown => self.scroll = self.scroll.saturating_sub(10),
            KeyCode::Up if !self.transcript.is_empty() => self.scroll += 1,
            KeyCode::Down => self.scroll = self.scroll.saturating_sub(1),
            _ => {
                Self::edit(&mut self.input, &mut self.cur, k, ctrl);
            }
        }
    }

    fn dash_key(&mut self, k: KeyEvent, ctrl: bool) {
        if self.dash.detail.is_some() {
            match k.code {
                KeyCode::Esc | KeyCode::Enter | KeyCode::Char('q') | KeyCode::Backspace => self.dash.detail = None,
                KeyCode::Down => self.dash.detail.as_mut().unwrap().2 += 1,
                KeyCode::Up => {
                    let d = self.dash.detail.as_mut().unwrap();
                    d.2 = d.2.saturating_sub(1);
                }
                _ => {}
            }
            return;
        }
        // tabs: left / right, tab / shift+tab (also on the Send form)
        match k.code {
            KeyCode::Tab | KeyCode::BackTab if !(self.dash.tab == dash::Tab::Send && self.typing() && k.code == KeyCode::Tab && self.dash.send.field == 0) => {
                self.dash.tab = self.dash.tab.next(k.code == KeyCode::BackTab);
                return;
            }
            KeyCode::Esc if !(self.dash.tab == dash::Tab::Send && (self.dash.send.confirm || self.dash.send.field == 1)) => {
                self.go(ViewId::Home);
                return;
            }
            _ => {}
        }
        if self.dash.tab == dash::Tab::Send {
            if !self.has_key {
                if let KeyCode::Left | KeyCode::Right = k.code {
                    self.dash.tab = self.dash.tab.next(k.code == KeyCode::Left);
                }
                return;
            }
            let f = &mut self.dash.send;
            if f.busy {
                return;
            }
            match k.code {
                KeyCode::Esc => {
                    if f.confirm {
                        f.confirm = false;
                    } else {
                        f.field = 0;
                    }
                }
                KeyCode::Tab | KeyCode::Down if !f.confirm => f.field = 1,
                KeyCode::Up if !f.confirm => f.field = 0,
                KeyCode::Enter => {
                    if f.confirm {
                        self.send_transfer();
                    } else if f.field == 0 {
                        f.field = 1;
                    } else if !f.to.is_empty() && !f.amount.is_empty() {
                        f.confirm = true;
                        f.result = None;
                    }
                }
                _ if !f.confirm => {
                    let mut cur = f.cur().len();
                    let field = f.cur();
                    if k.code == KeyCode::Left && cur == 0 {
                        return;
                    }
                    Self::edit(field, &mut cur, k, ctrl);
                }
                _ => {}
            }
            return;
        }
        let Some(sn) = &self.snap else {
            if let KeyCode::Left | KeyCode::Right = k.code {
                self.dash.tab = self.dash.tab.next(k.code == KeyCode::Left);
            }
            return;
        };
        let t = dash::table(sn, self.dash.tab);
        let page = self.dash.visible.get().max(1) as isize;
        match k.code {
            KeyCode::Left => self.dash.tab = self.dash.tab.next(true),
            KeyCode::Right => self.dash.tab = self.dash.tab.next(false),
            KeyCode::Up | KeyCode::Char('k') => self.dash.move_sel(&t, -1),
            KeyCode::Down | KeyCode::Char('j') => self.dash.move_sel(&t, 1),
            KeyCode::PageUp => self.dash.move_sel(&t, -page),
            KeyCode::PageDown => self.dash.move_sel(&t, page),
            KeyCode::Home | KeyCode::Char('g') => self.dash.move_sel(&t, -(t.keys.len() as isize)),
            KeyCode::End | KeyCode::Char('G') => self.dash.move_sel(&t, t.keys.len() as isize),
            KeyCode::Enter => self.dash.open_detail(&t),
            KeyCode::Char('r') => self.kick(poll::Kick::Now),
            KeyCode::Char('s') => self.dash.tab = dash::Tab::Send,
            _ => {}
        }
    }

    fn agents_key(&mut self, k: KeyEvent) {
        let n = self.agents.len();
        if n == 0 {
            if k.code == KeyCode::Esc {
                self.go(ViewId::Home);
            }
            return;
        }
        let i = self.agents_v.index(&self.agents);
        let mv = |d: isize| (i as isize + d).clamp(0, n as isize - 1) as usize;
        let j = match k.code {
            KeyCode::Up | KeyCode::Char('k') => mv(-1),
            KeyCode::Down | KeyCode::Char('j') => mv(1),
            KeyCode::PageUp => mv(-10),
            KeyCode::PageDown => mv(10),
            KeyCode::Home | KeyCode::Char('g') => 0,
            KeyCode::End | KeyCode::Char('G') => n - 1,
            KeyCode::Char(']') => {
                self.agents_v.card_scroll += 3;
                return;
            }
            KeyCode::Char('[') => {
                self.agents_v.card_scroll = self.agents_v.card_scroll.saturating_sub(3);
                return;
            }
            KeyCode::Char('r') | KeyCode::Enter => {
                // read the card again
                let a = self.agents[i].clone();
                self.agents_v.cards.remove(&a.account);
                self.load_card();
                return;
            }
            KeyCode::Char('m') => {
                // talk to this agent on Home (Ask's Tab mode)
                self.mode = i + 1;
                self.quote = None;
                let a = self.agents[i].clone();
                self.show_agent_card(&a);
                self.go(ViewId::Home);
                return;
            }
            KeyCode::Esc => {
                self.go(ViewId::Home);
                return;
            }
            _ => return,
        };
        self.agents_v.sel = Some(self.agents[j].account.clone());
        self.agents_v.card_scroll = 0;
        self.load_card();
    }

    fn inbox_key(&mut self, k: KeyEvent) {
        let threads: Vec<Value> = self.inbox.as_ref().and_then(|r| r.as_ref().ok()).and_then(|v| v["threads"].as_array().cloned()).unwrap_or_default();
        let peer = |t: &Value| crate::snapshot::s(&t["peer"], "account").to_string();
        let i = self.inbox_v.sel.as_ref().and_then(|p| threads.iter().position(|t| &peer(t) == p)).unwrap_or(0);
        match k.code {
            KeyCode::Esc => self.go(ViewId::Home),
            KeyCode::Up | KeyCode::Char('k') if i > 0 => self.inbox_v.sel = Some(peer(&threads[i - 1])),
            KeyCode::Down | KeyCode::Char('j') if i + 1 < threads.len() => self.inbox_v.sel = Some(peer(&threads[i + 1])),
            KeyCode::Enter | KeyCode::Char('r') if !threads.is_empty() => {
                let (Some(api), Some(me)) = (self.net.api.clone(), self.me.clone()) else { return };
                let p = peer(&threads[i]);
                self.inbox_v.sel = Some(p.clone());
                let t = self.tx.clone();
                std::thread::spawn(move || {
                    let r = net::get(&format!("{api}/threads/{me}/{p}?limit=50")).map_err(|e| format!("{e:#}"));
                    let _ = t.send(Msg::Thread(p, r));
                });
            }
            KeyCode::Char('m') if !threads.is_empty() => {
                let p = &threads[i]["peer"];
                let acct = peer(&threads[i]);
                let a = match self.agents.iter().position(|a| a.account == acct) {
                    Some(j) => {
                        self.mode = j + 1;
                        self.agents[j].clone()
                    }
                    None => {
                        let a = AgentEntry { account: acct.clone(), handle: p["handle"].as_str().unwrap_or(&crate::fmt::short(&acct)).to_string(), name: p["name"].as_str().unwrap_or("").to_string(), house: p["house"] == true, gh: net::github_of(&p["links"]) };
                        self.agents.push(a.clone());
                        self.mode = self.agents.len();
                        a
                    }
                };
                self.show_agent_card(&a);
                self.go(ViewId::Home);
            }
            _ => {}
        }
    }

    fn activity_key(&mut self, k: KeyEvent) {
        let a = &mut self.activity;
        let n = a.visible().len();
        match k.code {
            KeyCode::Esc => self.go(ViewId::Home),
            KeyCode::Up | KeyCode::Char('k') => a.scroll = a.scroll.saturating_sub(1),
            KeyCode::Down | KeyCode::Char('j') => a.scroll = (a.scroll + 1).min(n.saturating_sub(1)),
            KeyCode::PageUp => a.scroll = a.scroll.saturating_sub(15),
            KeyCode::PageDown => a.scroll = (a.scroll + 15).min(n.saturating_sub(1)),
            KeyCode::Home | KeyCode::Char('g') => a.scroll = 0,
            KeyCode::End | KeyCode::Char('G') => a.scroll = n.saturating_sub(1),
            KeyCode::Char('r') => self.kick(poll::Kick::Now),
            KeyCode::Char(c) => {
                if let Some(f) = views::Filter::ALL.into_iter().find(|f| f.key() == c) {
                    if a.filter != f {
                        a.filter = f;
                        a.scroll = 0;
                    }
                }
            }
            _ => {}
        }
    }

    fn boards_key(&mut self, k: KeyEvent) {
        let b = &mut self.boards_v;
        let mut reread = false;
        match k.code {
            KeyCode::Esc => return self.go(ViewId::Home),
            KeyCode::Tab | KeyCode::Right | KeyCode::Char('l') => b.next_board(1),
            KeyCode::BackTab | KeyCode::Left => b.next_board(-1),
            KeyCode::Up | KeyCode::Char('k') => b.scroll = b.scroll.saturating_sub(1),
            KeyCode::Down | KeyCode::Char('j') => b.scroll += 1,
            KeyCode::Char('w') => {
                b.cycle_window();
                reread = true;
            }
            KeyCode::Char('h') | KeyCode::Char('s') => {
                b.cycle_side();
                reread = true;
            }
            KeyCode::Char('r') => reread = true,
            _ => {}
        }
        if reread {
            let (w, s) = (b.window as u8, b.side as u8);
            if let Some(p) = &self.poll {
                p.wants.boards_window.store(w, Ordering::Relaxed);
                p.wants.boards_side.store(s, Ordering::Relaxed);
            }
            self.kick(poll::Kick::One(Source::Boards));
        }
    }

    fn trade_key(&mut self, k: KeyEvent) {
        let t = &mut self.trade_v;
        match k.code {
            KeyCode::Esc => self.go(ViewId::Home),
            KeyCode::Up | KeyCode::Char('k') => t.scroll = t.scroll.saturating_sub(1),
            KeyCode::Down | KeyCode::Char('j') => t.scroll += 1,
            KeyCode::PageUp => t.scroll = t.scroll.saturating_sub(15),
            KeyCode::PageDown => t.scroll += 15,
            KeyCode::Home | KeyCode::Char('g') => t.scroll = 0,
            KeyCode::Char('r') => self.kick(poll::Kick::One(Source::Trade)),
            _ => {}
        }
    }

    fn status_key(&mut self, k: KeyEvent) {
        match k.code {
            KeyCode::Esc => self.go(ViewId::Home),
            KeyCode::Up | KeyCode::Char('k') => self.status_scroll = self.status_scroll.saturating_sub(1),
            KeyCode::Down | KeyCode::Char('j') => self.status_scroll += 1,
            KeyCode::PageUp => self.status_scroll = self.status_scroll.saturating_sub(15),
            KeyCode::PageDown => self.status_scroll += 15,
            KeyCode::Home | KeyCode::Char('g') => self.status_scroll = 0,
            KeyCode::Char('r') => self.kick(poll::Kick::One(Source::Status)),
            _ => {}
        }
    }

    fn lookup_key(&mut self, k: KeyEvent, ctrl: bool) {
        match k.code {
            KeyCode::Esc => {
                if self.lookup_input.is_empty() {
                    self.go(ViewId::Home);
                } else {
                    self.lookup_input.clear();
                    self.lookup_cur = 0;
                }
            }
            KeyCode::Enter => {
                let line: String = self.lookup_input.iter().collect::<String>().trim().to_string();
                if line.is_empty() {
                    return;
                }
                self.lookup_input.clear();
                self.lookup_cur = 0;
                match line.strip_prefix('/') {
                    Some(c) => {
                        let w = split_words(c);
                        if !w.is_empty() {
                            self.run_words(w);
                        }
                    }
                    None => self.run_lookup(line, false),
                }
            }
            KeyCode::PageUp => self.lookup_off -= 10,
            KeyCode::PageDown => self.lookup_off += 10,
            KeyCode::Up => self.lookup_off -= 1,
            KeyCode::Down => self.lookup_off += 1,
            KeyCode::Char('l') if ctrl => {
                self.lookups.clear();
                self.lookup_off = 0;
            }
            _ => {
                Self::edit(&mut self.lookup_input, &mut self.lookup_cur, k, ctrl);
            }
        }
    }

    fn paste(&mut self, s: &str) {
        let clean: Vec<char> = s.chars().map(|c| if c == '\n' || c == '\r' || c == '\t' { ' ' } else { c }).filter(|c| !c.is_control()).collect();
        let (input, cur) = match self.view {
            ViewId::Home => (&mut self.input, &mut self.cur),
            ViewId::Lookup => (&mut self.lookup_input, &mut self.lookup_cur),
            ViewId::Dashboard if self.typing() => {
                let f = &mut self.dash.send;
                let field = if f.field == 0 { &mut f.to } else { &mut f.amount };
                field.extend(clean);
                return;
            }
            _ => return,
        };
        for c in clean {
            input.insert(*cur, c);
            *cur += 1;
        }
    }

    /// The robot is on screen (so the loop redraws at its frame rate).
    fn animating(&self, w: u16, h: u16) -> bool {
        self.view == ViewId::Home && self.transcript.is_empty() && self.palette.is_none() && plan_home(w, h).robot != RobotSize::None
    }

    /// Something the user started is running (a spinner is on screen).
    fn spinning(&self) -> bool {
        self.busy.is_some()
            || self.quoting
            || self.dash.send.busy
            || self.lookups.iter().any(|l| l.out.is_none())
            || (self.view != ViewId::Home && self.snap.is_none())
            || (self.view == ViewId::Status && self.report.is_none())
            || (self.view == ViewId::Agents && (self.agents.is_empty() && self.agents_err.is_none()))
    }
}

/// A Lookup line: a block height, a tx hash or Solana signature, an address or @handle,
/// else a command line.
fn run_line(ctx: &Ctx, line: &str, is_cmd: bool) -> Result<crate::commands::Out> {
    use crate::commands::{exec, parse_line, Cmd};
    let q = line.trim();
    if !is_cmd && !q.contains(' ') {
        let hex = q.strip_prefix("0x").unwrap_or(q);
        if q.chars().all(|c| c.is_ascii_digit()) {
            return exec(ctx, &Cmd::Block { id: q.into() }, true);
        }
        if hex.len() == 64 && hex.chars().all(|c| c.is_ascii_hexdigit()) {
            // a tx hash, else a block hash
            return match exec(ctx, &Cmd::Tx { hash: q.into(), watch: false, timeout: 0 }, true) {
                Ok(o) => Ok(o),
                Err(e) => exec(ctx, &Cmd::Block { id: q.into() }, true).map_err(|_| e),
            };
        }
        if q.len() >= 80 && bs58::decode(q).into_vec().is_ok_and(|v| v.len() == 64) {
            return exec(ctx, &Cmd::Tx { hash: q.into(), watch: false, timeout: 0 }, true);
        }
        if q.starts_with('@') || (q.len() >= 32 && q.len() <= 48) {
            return exec(ctx, &Cmd::Account { address: q.into() }, true);
        }
    }
    let cmd = parse_line(q)?;
    match cmd {
        Cmd::Watch { .. } => anyhow::bail!("watch is the Activity view (5)"),
        Cmd::Dashboard => anyhow::bail!("the dashboard is view 2"),
        c => exec(ctx, &c, true),
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

/// Word wrap to `w` cells (long words are cut).
pub fn wrap(s: &str, w: usize) -> Vec<String> {
    draw::wrap(s, w)
}

// ---------------------------------------------------------------------------------------
// drawing: the frame every view shares

/// Rows of the frame at height `h`: (hints row, footer row).
fn frame_rows(h: u16) -> (u16, Option<u16>) {
    let f = home::footer_y(h);
    let hints = match home::footer_rows(h) {
        3 => h.saturating_sub(4),
        1 => h.saturating_sub(2),
        _ => h.saturating_sub(1),
    };
    (hints, f)
}

/// The body of a view other than Home.
fn body_rect(w: u16, h: u16) -> Rect {
    let (hints, _) = frame_rows(h);
    let top = if h >= 20 { 3 } else { 2 };
    Rect::new(2.min(w), top.min(h), w.saturating_sub(4), hints.saturating_sub(top + 1))
}

/// Small header: the plain wordmark, the view's name, and a live line on the right.
fn draw_header(buf: &mut Buffer, pen: &Pen, app: &App, w: u16) {
    let y = if buf.area.height >= 20 { 1 } else { 0 };
    let x = put(buf, 2, y, "cadre", pen.fg(MUTED), w);
    let x = put(buf, x, y, "agents", pen.fg(TEXT).add_modifier(Modifier::BOLD), w);
    let x = put(buf, x + 3, y, app.view.name(), pen.fg(BLUE).add_modifier(Modifier::BOLD), w);
    let mut parts: Vec<(String, u32)> = vec![];
    match &app.me {
        Some(a) => parts.push((format!("{} {}", if app.has_key { "wallet" } else { "watching" }, crate::fmt::short_addr(a)), MUTED)),
        None => parts.push(("no wallet".into(), DIM)),
    }
    if let Some(t) = app.tip() {
        parts.push((format!("block {t}"), TEXT));
    } else if app.snap.as_ref().is_some_and(|s| !s.chain_ok()) {
        parts.push(("chain offline".into(), theme::RED));
    }
    let src = match app.view {
        ViewId::Status => Source::Status,
        ViewId::Activity => Source::Feed,
        ViewId::Agents => Source::Agents,
        ViewId::Inbox => Source::Inbox,
        ViewId::Trade => Source::Trade,
        ViewId::Market => Source::Market,
        ViewId::Boards => Source::Boards,
        _ => Source::Chain,
    };
    if app.loading.contains(&src) && app.updated.get(&src).is_none() {
        parts.push((format!("{} reading", draw::spinner(app.start.elapsed().as_millis())), MUTED));
    } else if let Some(t) = app.updated.get(&src) {
        parts.push((format!("updated {}", draw::ago(t.elapsed().as_millis() as u64)), DIM));
    }
    let total: u16 = parts.iter().map(|(s, _)| str_w(s)).sum::<u16>() + 3 * parts.len().saturating_sub(1) as u16;
    let mut rx = w.saturating_sub(2 + total);
    if rx < x + 2 {
        return;
    }
    for (i, (s, c)) in parts.iter().enumerate() {
        if i > 0 {
            rx = put(buf, rx, y, " · ", pen.fg(DIM), w);
        }
        rx = put(buf, rx, y, s, pen.fg(*c), w);
    }
}

/// The hints row of a view: key (text) + what (muted) pairs, right-aligned.
fn draw_view_hints(buf: &mut Buffer, pen: &Pen, app: &App, y: u16, w: u16) {
    let mut parts: Vec<(&str, &str)> = match app.view {
        ViewId::Dashboard if app.dash.detail.is_some() => vec![("↑↓", "scroll"), ("esc", "close")],
        ViewId::Dashboard if app.dash.tab == dash::Tab::Send && app.dash.send.confirm => vec![("enter", "sign and send"), ("esc", "edit")],
        ViewId::Dashboard if app.dash.tab == dash::Tab::Send => vec![("enter", "next"), ("tab", "tabs"), ("f1-f9", "views")],
        ViewId::Dashboard => vec![("↑↓", "select"), ("enter", "open"), ("←→", "tab"), ("1-9", "views"), ("esc", "home")],
        ViewId::Agents => vec![("↑↓", "select"), ("enter", "reload"), ("[ ]", "scroll card"), ("m", "message"), ("esc", "home")],
        ViewId::Inbox => vec![("↑↓", "select"), ("enter", "open"), ("m", "reply on home"), ("esc", "home")],
        ViewId::Activity => vec![("a b t p x", "filter"), ("↑↓", "scroll"), ("g", "newest"), ("esc", "home")],
        ViewId::Status => vec![("↑↓", "scroll"), ("r", "check again"), ("esc", "home")],
        ViewId::Lookup => vec![("enter", "look up"), ("↑↓", "scroll"), ("f1-f9", "views"), ("esc", "home")],
        ViewId::Trade => vec![("↑↓", "scroll"), ("r", "read again"), ("esc", "home")],
        ViewId::Market => vec![("↑↓", "select"), ("tab", "offers / hires"), ("[ ]", "scroll card"), ("r", "read again"), ("esc", "home")],
        ViewId::Boards => vec![("tab ←→", "board"), ("w", "window"), ("h", "house side"), ("↑↓", "scroll"), ("esc", "home")],
        ViewId::Home => vec![],
    };
    parts.push(("ctrl+p", "commands"));
    let total: u16 = parts.iter().map(|(k, d)| str_w(k) + 1 + str_w(d)).sum::<u16>() + 2 * parts.len().saturating_sub(1) as u16;
    let mut x = w.saturating_sub(2 + total).max(2);
    for (i, (k, d)) in parts.iter().enumerate() {
        if i > 0 {
            x += 2;
        }
        x = put(buf, x, y, k, pen.fg(TEXT), w);
        x = put(buf, x + 1, y, d, pen.fg(MUTED), w);
    }
}

/// The bottom bar: directory and branch, the views (current one bright), network and version.
/// The footer's side texts: (cwd shown, its end column, network/version text, its start column).
fn footer_sides(app: &App, w: u16) -> (String, u16, String, u16) {
    let right_s = format!("{} · {}", app.network, env!("CARGO_PKG_VERSION"));
    let rx = w.saturating_sub(2 + str_w(&right_s));
    let cwd = clip(&app.cwd, rx.saturating_sub(4) as usize / 2);
    let cx = 2 + str_w(&cwd);
    (cwd, cx, right_s, rx)
}

/// Where each view sits in the footer strip: (from column, to column, view). Empty when the
/// strip does not fit. Shared by drawing and by mouse clicks.
fn strip_cells(app: &App, w: u16) -> Vec<(u16, u16, ViewId)> {
    let (_, cx, _, rx) = footer_sides(app, w);
    let label = |i: usize, v: &ViewId| str_w(&format!("{} {}", (i + 1) % 10, v.name()));
    let sw: u16 = ViewId::TOP.iter().enumerate().map(|(i, v)| label(i, v)).sum::<u16>() + 2 * (ViewId::TOP.len() as u16 - 1);
    let room_l = cx + 3;
    let room_r = rx.saturating_sub(3);
    if room_r <= room_l || room_r - room_l < sw {
        return vec![];
    }
    // centered on the screen, under the content; between the two side texts only when they would overlap
    let centered = w.saturating_sub(sw) / 2;
    let mut x = if centered >= room_l && centered + sw <= room_r { centered } else { room_l + (room_r - room_l - sw) / 2 };
    let mut out = vec![];
    for (i, v) in ViewId::TOP.iter().enumerate() {
        let lw = label(i, v);
        out.push((x, x + lw, *v));
        x += lw + 2;
    }
    out
}

fn draw_footer(buf: &mut Buffer, pen: &Pen, app: &App, y: u16, w: u16) {
    let (cwd, _, right_s, rx) = footer_sides(app, w);
    put(buf, 2, y, &cwd, pen.fg(MUTED), rx.saturating_sub(2));
    put(buf, rx, y, &right_s, pen.fg(MUTED), w);
    for (i, (x, _, v)) in strip_cells(app, w).into_iter().enumerate() {
        let on = v == app.view.group();
        let nx = put(buf, x, y, &((i + 1) % 10).to_string(), pen.fg(if on { BLUE } else { DIM }), w);
        put(buf, nx + 1, y, v.name(), if on { pen.fg(TEXT).add_modifier(Modifier::BOLD) } else { pen.fg(MUTED) }, w);
    }
}

/// The Agents sections bar: (from column, to column, section) on the body's first row.
fn section_cells(body: Rect) -> Vec<(u16, u16, ViewId)> {
    let mut x = body.x + 1;
    let mut out = vec![];
    for v in ViewId::SECTIONS {
        let lw = str_w(v.section_label());
        out.push((x, x + lw, v));
        x += lw + 4;
    }
    out
}

/// Town, hire, trade, boards: the things agents do, the current one bright, with the keys.
fn draw_sections(buf: &mut Buffer, pen: &Pen, app: &App, body: Rect) {
    let cells = section_cells(body);
    for (x, _, v) in &cells {
        let on = *v == app.view;
        put(buf, *x, body.y, v.section_label(), if on { pen.fg(TEXT).add_modifier(Modifier::BOLD | Modifier::UNDERLINED) } else { pen.fg(MUTED) }, body.right());
    }
    if let Some((_, end, _)) = cells.last() {
        put(buf, end + 4, body.y, ", . or 3: next", pen.fg(DIM), body.right());
    }
}

/// The Agents body below its sections bar.
fn below_sections(body: Rect) -> Rect {
    Rect::new(body.x, body.y + 2, body.width, body.height.saturating_sub(2))
}

/// `s` cut to `max` columns with a trailing ellipsis.
fn ellipsize(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let keep: String = s.chars().take(max.saturating_sub(1)).collect();
    format!("{}…", keep.trim_end())
}

fn draw_palette(buf: &mut Buffer, pen: &Pen, app: &App) {
    let a = buf.area;
    // wide enough for the descriptions; rows: title, filter, rule, the list, a bottom pad
    let w = a.width.saturating_sub(4).min(96);
    let items = app.palette_items();
    let h = a.height.saturating_sub(4).min(items.len() as u16 + 6).clamp(7.min(a.height), 26);
    let x = (a.width - w) / 2;
    let y = (a.height.saturating_sub(h)) / 3;
    let r = Rect::new(x, y, w, h);
    fill(buf, r, pen.bg(PANEL));
    let right = x + w;
    put(buf, x + 2, y + 1, "Commands", pen.on(TEXT, PANEL).add_modifier(Modifier::BOLD), right);
    put(buf, right.saturating_sub(5), y + 1, "esc", pen.on(SUBTLE, PANEL), right);
    let p = app.palette.as_ref().unwrap();
    // how many things there are to do, or how many match the filter
    let total = app.palette_total();
    let count = if p.filter.is_empty() { format!("{total} things you can do") } else { format!("{} of {total}", items.len()) };
    put(buf, x + 12, y + 1, &count, pen.on(SUBTLE, PANEL), right.saturating_sub(7));
    let sx = put(buf, x + 2, y + 2, "> ", pen.on(BLUE, PANEL), right);
    if p.filter.is_empty() {
        put(buf, sx, y + 2, "Type to filter views and commands", pen.on(SUBTLE, PANEL), right);
    } else {
        put(buf, sx, y + 2, &p.filter, pen.on(TEXT, PANEL), right);
    }
    put(buf, x + 2, y + 3, &"─".repeat(w.saturating_sub(4) as usize), pen.on(DIM, PANEL), right);
    let rows = h.saturating_sub(5) as usize;
    let sel = p.sel.min(items.len().saturating_sub(1));
    let first = sel.saturating_sub(rows.saturating_sub(1));
    // name column: the longest name, plus a gap
    let name_w = items.iter().map(|c| c.name().chars().count()).max().unwrap_or(10).min(20) as u16 + 3;
    for (i, c) in items.iter().enumerate().skip(first).take(rows) {
        let yy = y + 4 + (i - first) as u16;
        let on = i == sel;
        if on {
            fill(buf, Rect::new(x + 1, yy, w - 2, 1), pen.bg(PRIMARY));
        }
        let (fg, fg2, bg) = if on { (BG, BG, PRIMARY) } else { (TEXT, SUBTLE, PANEL) };
        let mut st = pen.on(fg, bg);
        if on && pen.mode == Mode::NoColor {
            st = st.add_modifier(Modifier::REVERSED);
        }
        let nx = put(buf, x + 2, yy, &c.name(), st, right);
        let about = c.about();
        let about = about.split(['.', ':']).next().unwrap_or("").trim().to_string();
        let about = if let Item::View(v) = c { format!("view {}  {}", v.num(), v.about()) } else { about };
        let ax = (x + 2 + name_w).max(nx + 2);
        let room = right.saturating_sub(2).saturating_sub(ax) as usize;
        put(buf, ax, yy, &ellipsize(&about, room), pen.on(fg2, bg), right.saturating_sub(1));
    }
    if items.is_empty() {
        put(buf, x + 2, y + 4, "no view or command matches", pen.on(SUBTLE, PANEL), right);
    }
}

fn draw(f: &mut ratatui::Frame, app: &App) {
    let area = f.area();
    let (w, h) = (area.width, area.height);
    let pen = Pen { mode: app.color };
    let buf = f.buffer_mut();
    // the terminal's own background everywhere; only cards, selected rows, the prompt and the palette paint one
    fill(buf, area, Style::default().bg(ratatui::style::Color::Reset));
    let ms = app.start.elapsed().as_millis();
    let mut cursor = None;
    if app.view == ViewId::Home {
        cursor = home::draw(buf, &pen, app, w, h);
    } else {
        draw_header(buf, &pen, app, w);
        let mut body = body_rect(w, h);
        if app.view.group() == ViewId::Agents && body.height > 6 {
            draw_sections(buf, &pen, app, body);
            body = below_sections(body);
        }
        let (hints_y, _) = frame_rows(h);
        match app.view {
            ViewId::Dashboard => {
                let cx = dash::Ctx { snap: app.snap.as_ref(), has_key: app.has_key, min_fee: app.snap.as_ref().filter(|s| s.params.is_object()).map(|s| crate::snapshot::u(&s.params, "min_fee")), ms };
                dash::draw(buf, &pen, body, &app.dash, &cx);
                if let Some(d) = &app.dash.detail {
                    dash::draw_detail(buf, &pen, area, d);
                } else if app.dash.tab == dash::Tab::Send && app.typing() {
                    cursor = send_cursor(app, body);
                }
            }
            ViewId::Agents => views::draw_agents(buf, &pen, body, &app.agents, app.agents_err.as_ref(), &app.agents_v, &app.dm_status(), ms),
            ViewId::Inbox => views::draw_inbox(buf, &pen, body, app.me.as_deref(), &app.dm_status(), app.inbox.as_ref(), &app.inbox_v, ms),
            ViewId::Activity => {
                let mut errs: Vec<String> = app.feed_errors.clone();
                if let Some(e) = app.snap.as_ref().and_then(|s| s.errors.first()) {
                    errs.push(e.clone());
                }
                let loading = app.loading.contains(&Source::Chain) || app.loading.contains(&Source::Feed);
                views::draw_activity(buf, &pen, body, &app.activity, &errs, ms, loading);
            }
            ViewId::Status => draw_status(buf, &pen, app, body, ms),
            ViewId::Lookup => cursor = draw_lookup(buf, &pen, app, body, ms),
            ViewId::Trade => trade::draw_trade(buf, &pen, body, &app.trade_v, app.me.as_deref(), ms, app.loading.contains(&Source::Trade)),
            ViewId::Market => crate::market::draw_view(buf, &pen, body, app.market.as_ref(), &app.market_v, ms),
            ViewId::Boards => boards_view::draw_boards(buf, &pen, body, &app.boards_v, ms, app.loading.contains(&Source::Boards)),
            ViewId::Home => {}
        }
        if hints_y > body.y {
            draw_view_hints(buf, &pen, app, hints_y, w);
        }
    }
    if let (_, Some(fy)) = frame_rows(h) {
        draw_footer(f.buffer_mut(), &pen, app, fy, w);
    }
    if app.palette.is_some() {
        draw_palette(f.buffer_mut(), &pen, app);
        cursor = None;
    }
    if let Some(c) = cursor {
        f.set_cursor_position(c);
    }
}

fn send_cursor(app: &App, body: Rect) -> Option<Position> {
    let f = &app.dash.send;
    if f.confirm || f.busy {
        return None;
    }
    // the table area starts under the cards (dash::draw) and the tab bar
    let ch = dash::card_height(body.height);
    let y0 = body.y + ch + 1 + 2;
    let x = body.x + 2 + 10 + 2;
    let v = if f.field == 0 { &f.to } else { &f.amount };
    Some(Position::new(x + str_w(&v.iter().collect::<String>()), y0 + 2 * f.field as u16))
}

fn draw_status(buf: &mut Buffer, pen: &Pen, app: &App, r: Rect, ms: u128) {
    let Some(rep) = &app.report else {
        put(buf, r.x + 2, r.y, &format!("{} checking every component (nodes, Solana, light client, relays, services, agents, economy)", draw::spinner(ms)), pen.fg(MUTED), r.right());
        return;
    };
    let mut x = r.x;
    for (s, c) in views::status_summary(rep) {
        x = put(buf, x, r.y, &s, pen.fg(c), r.right());
    }
    if app.loading.contains(&Source::Status) {
        put_right(buf, r.right(), r.y, &format!("{} checking again", draw::spinner(ms)), pen.fg(MUTED), x + 2);
    }
    let body = Rect::new(r.x, r.y + 2, r.width, r.height.saturating_sub(2));
    let c = views::status_canvas(pen, body.width, rep);
    let top = app.status_scroll.min(views::max_scroll(c.area.height, body.height));
    views::blit(&c, buf, body, top);
}

fn draw_lookup(buf: &mut Buffer, pen: &Pen, app: &App, r: Rect, ms: u128) -> Option<Position> {
    // the prompt at the bottom, results above it (newest at the bottom, like Home)
    let by = r.bottom().saturating_sub(home::BOX_ROWS);
    let mode = vec![("Lookup".to_string(), BLUE), ("tx · block · account · @handle · /command".to_string(), MUTED)];
    let ph = "a tx hash, a block height or hash, an address or @handle, or /command";
    let cursor = home::prompt_box(buf, pen, r.x, by, r.width, &app.lookup_input, app.lookup_cur, ph, &mode);
    let area = Rect::new(r.x, r.y, r.width, by.saturating_sub(r.y + 1));
    if app.lookups.is_empty() {
        let lines = [
            "Type what to look up and press enter:",
            "  4768                 a block by height",
            "  9f2c...e1 (64 hex)    a Cadre tx, else a block by hash",
            "  5Kx...  (signature)   a Solana tx, decoded in words",
            "  @tape_reader          an account or agent (or its address)",
            "  /job show 66f8        any cadre command, drawn here",
        ];
        for (i, l) in lines.iter().enumerate() {
            put(buf, area.x + 2, area.y + i as u16, l, pen.fg(if i == 0 { TEXT } else { MUTED }), area.right());
        }
        return cursor;
    }
    let (c, last) = views::lookup_canvas(pen, area.width, &app.lookups, ms);
    let ch = c.area.height;
    // the newest result from its first line (or the bottom, when it all fits), moved by the scroll
    let auto = last.min(ch.saturating_sub(area.height));
    let top = (auto as i32 + app.lookup_off).clamp(0, ch.saturating_sub(area.height) as i32) as u16;
    let shown = Rect::new(area.x, area.y + area.height.saturating_sub(ch), area.width, area.height.min(ch));
    views::blit(&c, buf, shown, top);
    cursor
}

/// The Dashboard of `sn` rendered once into a buffer `w` x `h` with `tab` open, for
/// `cadre snapshot` (an HTML / text picture of the live dashboard; no terminal needed).
pub fn dashboard_buffer(sn: &Snapshot, network: &str, w: u16, h: u16, tab: dash::Tab) -> Buffer {
    let mut buf = Buffer::empty(Rect::new(0, 0, w, h));
    let pen = Pen { mode: Mode::TrueColor };
    let y = if h >= 20 { 1 } else { 0 };
    let x = put(&mut buf, 2, y, "cadre", pen.fg(MUTED), w);
    let x = put(&mut buf, x, y, "agents", pen.fg(TEXT).add_modifier(Modifier::BOLD), w);
    put(&mut buf, x + 3, y, "dashboard", pen.fg(BLUE).add_modifier(Modifier::BOLD), w);
    let who = match &sn.me {
        Some(a) => format!("{} · block {}", crate::fmt::short_addr(a), sn.height()),
        None => format!("no wallet · block {}", sn.height()),
    };
    put_right(&mut buf, w.saturating_sub(2), y, &who, pen.fg(MUTED), x + 14);
    let st = dash::DashState { tab, ..Default::default() };
    let cx = dash::Ctx { snap: Some(sn), has_key: false, min_fee: sn.params.is_object().then(|| crate::snapshot::u(&sn.params, "min_fee")), ms: 0 };
    dash::draw(&mut buf, &pen, body_rect(w, h), &st, &cx);
    if let (_, Some(fy)) = frame_rows(h) {
        put_right(&mut buf, w.saturating_sub(2), fy, &format!("{network} · {}", env!("CARGO_PKG_VERSION")), pen.fg(MUTED), 2);
    }
    buf
}

// ---------------------------------------------------------------------------------------
// terminal

fn restore() {
    let _ = terminal::disable_raw_mode();
    let _ = execute!(std::io::stdout(), DisableMouseCapture, DisableBracketedPaste, terminal::LeaveAlternateScreen, cursor::SetCursorStyle::DefaultUserShape, cursor::Show);
}

fn enter() -> Result<()> {
    terminal::enable_raw_mode()?;
    execute!(std::io::stdout(), terminal::EnterAlternateScreen, EnableBracketedPaste, EnableMouseCapture, cursor::SetCursorStyle::SteadyBlock)?;
    Ok(())
}

struct Guard;
impl Drop for Guard {
    fn drop(&mut self) {
        restore();
    }
}

/// The app; returns the exit code.
pub fn run(l: Launch) -> Result<i32> {
    let ctx = Arc::new(Ctx::new(l.opts.clone())?);
    let net = net_of(&ctx);
    let mut app = App::new(l, ctx.clone(), net.clone());
    app.poll = Some(poll::start(ctx, net, app.me.clone(), app.tx.clone()));
    if app.view == ViewId::Status {
        app.go(ViewId::Status);
    }
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
    let spin = Duration::from_millis(100);
    let tick = Duration::from_secs(1);
    let mut last = Instant::now() - tick;
    let mut dirty = true;
    while !app.quit {
        let size = term.size()?;
        // the robot at its frame rate, spinners at 10 fps, the "updated N s ago" clock at 1 Hz
        let period = if app.animating(size.width, size.height) {
            frame
        } else if app.spinning() {
            spin
        } else {
            tick
        };
        if dirty || last.elapsed() >= period {
            term.draw(|f| draw(f, &app))?;
            last = Instant::now();
            dirty = false;
        }
        let wait = period.saturating_sub(last.elapsed()).min(Duration::from_millis(250));
        if event::poll(wait)? {
            loop {
                match event::read()? {
                    Event::Key(k) => app.key(k),
                    Event::Paste(s) => app.paste(&s),
                    Event::Mouse(m) => {
                        let (w, h) = terminal::size().unwrap_or((80, 24));
                        app.mouse(m, w, h);
                    }
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
    }
    Ok(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::snapshot::sample;
    use home::{plan_chat, LogoKind, BOX_ROWS};

    #[test]
    fn opens_only_for_cadre_alone_on_a_terminal() {
        assert!(should_open("cadre", false, false, true, true, Some("xterm-256color")));
        // every other case keeps the old behavior
        assert!(!should_open("other", false, false, true, true, Some("xterm")));
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
        let p = plan_home(80, 30);
        assert_eq!((p.robot, p.logo), (RobotSize::Half, LogoKind::Wide));
        assert!(p.hints_y < 28);
        let p = plan_home(40, 40);
        assert_eq!((p.robot, p.logo), (RobotSize::Half, LogoKind::Stacked));
        let p = plan_home(28, 14);
        assert_eq!((p.robot, p.logo), (RobotSize::None, LogoKind::Text));
        assert!(p.hints_y < 14);
        for w in 0..20 {
            for h in 0..20 {
                let _ = plan_home(w, h);
                let _ = plan_chat(w, h);
                let _ = body_rect(w, h);
            }
        }
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

    fn render(app: &App, w: u16, h: u16) -> Buffer {
        use ratatui::backend::TestBackend;
        let mut t = Terminal::new(TestBackend::new(w, h)).unwrap();
        t.draw(|f| draw(f, app)).unwrap();
        t.backend().buffer().clone()
    }

    fn rows(b: &Buffer) -> Vec<String> {
        (0..b.area.height).map(|y| (0..b.area.width).map(|x| b[(x, y)].symbol().to_string()).collect::<String>()).collect()
    }

    /// The whole Home screen renders into a buffer at the reference sizes without panics,
    /// and the wordmark, prompt and hints are where the plan says.
    /// Owner 2026-10-21: palette "hard to read" and the view strip "should be aligned": the
    /// strip is centered on the screen (under the content) and descriptions end whole or with
    /// an ellipsis, never cut at the box edge.
    #[test]
    fn view_strip_centered_and_palette_readable() {
        // widths where the centered strip clears the side texts (narrower: it fits between them);
        // with nine views (Trade 8, Market 9) that starts at about 128 columns
        for (w, h) in [(140u16, 36u16), (160, 50)] {
            let mut app = test_app();
            app.cwd = "~".into();
            let b = render(&app, w, h);
            let fy = home::footer_y(h).unwrap();
            let row: Vec<char> = rows(&b)[fy as usize].chars().collect();
            let text: String = row.iter().collect();
            let at = |pat: &str| text.char_indices().position(|(i, _)| text[i..].starts_with(pat)).unwrap();
            let last = format!("{} {}", ViewId::TOP[ViewId::TOP.len() - 1].key_label(), ViewId::TOP[ViewId::TOP.len() - 1].name());
            let (start, end) = (at("1 home"), at(&last) + last.chars().count());
            let (l, r) = (start as i32, row.len() as i32 - end as i32);
            let row = text;
            assert!((l - r).abs() <= 1, "{w}x{h}: strip not centered, {l} left vs {r} right: {row:?}");
            app.key(KeyEvent::new(KeyCode::Char('p'), KeyModifiers::CONTROL));
            let b = render(&app, w, h);
            let all = rows(&b);
            assert!(all.iter().any(|r| r.contains("Type to filter views and commands")), "{w}x{h}");
            let total = app.palette_total();
            assert!(all.iter().any(|r| r.contains(&format!("{total} things you can do"))), "{w}x{h}: count missing");
            for it in app.palette_items().iter().take(8) {
                let about = it.about();
                let about = about.split(['.', ':']).next().unwrap_or("").trim().to_string();
                let line = all.iter().find(|r| r.contains(&format!(" {} ", it.name()))).unwrap_or_else(|| panic!("{} missing", it.name()));
                let shown = line[line.find(&it.name()).unwrap() + it.name().len()..].trim_end().trim_end_matches(' ');
                let shown = shown.trim();
                // view rows read "view N  <about>"; command rows show the about's first sentence
                let body = match shown.split_once("  ") {
                    Some((v, rest)) if v.starts_with("view ") => rest.trim(),
                    _ => shown,
                };
                let full = match it {
                    Item::View(v) => v.about().to_string(),
                    _ => it.about().to_string(),
                };
                assert!(body.ends_with('…') || body == about || full.starts_with(body), "{w}x{h}: {} -> {shown:?}", it.name());
            }
        }
        assert_eq!(ellipsize("Health of everything", 10), "Health of…");
    }

    #[test]
    fn renders_home_screen_into_a_buffer() {
        for (w, h) in [(120u16, 40u16), (80, 30), (40, 20), (24, 12), (10, 5), (1, 1)] {
            let app = test_app();
            let b = render(&app, w, h);
            let row = |y: u16| rows(&b)[y as usize].clone();
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

    /// Every view renders at the proof sizes (and tiny ones) without panics, with no
    /// painted background outside cards, selected rows, prompts and the palette, and no
    /// em dash anywhere.
    #[test]
    fn every_view_renders() {
        let mut app = test_app();
        app.snap = Some(sample::snapshot());
        app.apply(Msg::Snap(Box::new(sample::snapshot())));
        for v in ViewId::ALL {
            app.go(v);
            for (w, h) in [(120u16, 40u16), (160, 50), (80, 30), (30, 10), (5, 3)] {
                let b = render(&app, w, h);
                let all = rows(&b).join("\n");
                assert!(!all.contains('\u{2014}'), "{v:?} {w}x{h}");
                if w >= 80 && h >= 20 {
                    assert!(all.contains(env!("CARGO_PKG_VERSION")), "{v:?} footer {w}x{h}");
                    if v != ViewId::Home {
                        assert!(all.contains(v.name()), "{v:?} header {w}x{h}");
                    }
                }
            }
        }
        // the dashboard shows the sample's figures, and no beta banner
        app.go(ViewId::Dashboard);
        let all = rows(&render(&app, 120, 40)).join("\n");
        for want in ["40.74 CADRE", "8.69M CADRE", "job from 0.01 SOL", "block 260", "OK (in flight)", "demo-agent", "bbbb...bbbb", "open", "devnet · "] {
            assert!(all.contains(want), "{want} missing:\n{all}");
        }
        assert!(!all.contains("beta version"));
        assert!(!all.contains("$$$"));
    }

    /// The dashboard's palette: the blue bar on cards, the selected row on the panel color
    /// with the bar, the rest on the terminal's own background.
    #[test]
    fn dashboard_style() {
        let mut app = test_app();
        app.apply(Msg::Snap(Box::new(sample::snapshot())));
        app.go(ViewId::Dashboard);
        let b = render(&app, 120, 40);
        let r = rows(&b);
        let panel = Mode::TrueColor.color(PANEL);
        let blue = Mode::TrueColor.color(BLUE);
        // a card: bar + panel
        let (y, _) = r.iter().enumerate().find(|(_, l)| l.contains("Wallet")).unwrap();
        assert_eq!(b[(2, y as u16)].symbol(), "┃");
        assert_eq!(b[(2, y as u16)].fg, blue);
        assert_eq!(b[(10, y as u16)].bg, panel);
        // the selected row: the first job
        let (y, l) = r.iter().enumerate().find(|(_, l)| l.contains("bbbb...bbbb")).unwrap();
        let x = l.chars().position(|c| c == 'b').unwrap() as u16;
        assert_eq!(b[(x, y as u16)].bg, panel);
        let (y2, l2) = r.iter().enumerate().find(|(_, l)| l.contains("aaaa...aaaa")).unwrap();
        let x2 = l2.chars().position(|c| c == 'a').unwrap() as u16;
        assert_eq!(b[(x2, y2 as u16)].bg, ratatui::style::Color::Reset, "unselected rows sit on the terminal background");
        // the header row and the space between cards are never painted
        for x in 0..120 {
            assert_eq!(b[(x, 0)].bg, ratatui::style::Color::Reset);
        }
    }

    /// Boards is the last section of Agents (owner 2026-10-22: hire, trade and boards are things
    /// agents do): key 3 opens Agents and steps through its sections; boards still parses.
    #[test]
    fn boards_is_a_section_of_agents() {
        assert_eq!(ViewId::from_num(10), None);
        assert_eq!(ViewId::Boards.num(), 3);
        assert_eq!(ViewId::Boards.group(), ViewId::Agents);
        assert_eq!(ViewId::parse("top"), Some(ViewId::Boards));
        assert_eq!(ViewId::parse("leaderboard"), Some(ViewId::Trade));
        let k = |c: KeyCode, m: KeyModifiers| KeyEvent::new(c, m);
        let mut a = test_app();
        a.key(k(KeyCode::F(2), KeyModifiers::NONE));
        for _ in 0..4 {
            a.key(k(KeyCode::Char('3'), KeyModifiers::NONE));
        }
        assert_eq!(a.view, ViewId::Boards);
        a.key(k(KeyCode::Char('w'), KeyModifiers::NONE));
        assert_eq!(a.boards_v.window(), "all");
    }

    /// Owner 2026-10-21: "if i toggle the dashboard it shouldn't reset". Ask something on
    /// Home, go to the Dashboard and select a job, go Home and back: the conversation, its
    /// scroll, the input being typed, the tab, the selected job (also after new jobs
    /// arrive), and the Activity filter are all still there.
    #[test]
    fn switching_views_keeps_everything() {
        let k = |c: KeyCode, m: KeyModifiers| KeyEvent::new(c, m);
        let mut a = test_app();
        a.apply(Msg::Snap(Box::new(sample::snapshot())));
        // a conversation on Home, scrolled up a line, with a half-typed question
        a.transcript.push(Entry::new(Kind::Note, "first answer"));
        let mut you = Entry::new(Kind::You, "what is the tip?");
        you.label = "Ask".into();
        a.transcript.push(you);
        a.transcript.push(Entry::new(Kind::Answer, "block 260"));
        a.scroll = 1;
        for c in "and the fin".chars() {
            a.key(k(KeyCode::Char(c), KeyModifiers::NONE));
        }
        // Dashboard by F2: scroll down to the second job
        a.key(k(KeyCode::F(2), KeyModifiers::NONE));
        assert_eq!(a.view, ViewId::Dashboard);
        a.key(k(KeyCode::Down, KeyModifiers::NONE));
        let job = "aa".repeat(32);
        assert_eq!(a.dash.selected_key(), Some(&job));
        // a filter on Activity, then Home by Esc
        a.key(k(KeyCode::Char('5'), KeyModifiers::NONE));
        a.key(k(KeyCode::Char('p'), KeyModifiers::NONE));
        assert_eq!(a.activity.filter, views::Filter::Posts);
        a.key(k(KeyCode::Char('2'), KeyModifiers::NONE));
        a.key(k(KeyCode::Esc, KeyModifiers::NONE));
        assert_eq!(a.view, ViewId::Home);
        // Home: everything as it was
        assert_eq!(a.transcript.len(), 3);
        assert_eq!(a.scroll, 1);
        assert_eq!(a.input.iter().collect::<String>(), "and the fin");
        // new data arrives while on Home: a new job is posted
        let mut sn = sample::snapshot();
        sn.jobs.push(json!({"job_id": "cc".repeat(32), "status": "open", "m": 8, "n": 8, "k": 2048}));
        sn.jobs_total = 3;
        a.apply(Msg::Snap(Box::new(sn)));
        // back to the Dashboard: Esc on an empty prompt toggles, so clear the input first
        a.key(k(KeyCode::Char('u'), KeyModifiers::CONTROL));
        a.key(k(KeyCode::Esc, KeyModifiers::NONE));
        assert_eq!(a.view, ViewId::Dashboard);
        assert_eq!(a.dash.tab, dash::Tab::Jobs);
        assert_eq!(a.dash.selected_key(), Some(&job), "the same job, not the same row number");
        let all = rows(&render(&a, 120, 40)).join("\n");
        let line = all.lines().find(|l| l.contains("aaaa...aaaa")).unwrap().to_string();
        let b = render(&a, 120, 40);
        let y = rows(&b).iter().position(|l| *l == line).unwrap() as u16;
        let x = line.chars().position(|c| c == 'a').unwrap() as u16;
        assert_eq!(b[(x, y)].bg, Mode::TrueColor.color(PANEL), "drawn selected");
        // Activity kept its filter; Home kept the conversation
        a.key(k(KeyCode::Char('5'), KeyModifiers::NONE));
        assert_eq!(a.activity.filter, views::Filter::Posts);
        a.key(k(KeyCode::F(1), KeyModifiers::NONE));
        assert_eq!((a.transcript.len(), a.scroll), (3, 1));
        // the same through the palette and a /command
        a.palette = Some(Palette { filter: "dashboard".into(), sel: 0 });
        a.key(k(KeyCode::Enter, KeyModifiers::NONE));
        assert_eq!(a.view, ViewId::Dashboard);
        assert_eq!(a.dash.selected_key(), Some(&job));
        a.key(k(KeyCode::Char('1'), KeyModifiers::NONE));
        for c in "/dashboard".chars() {
            a.key(k(KeyCode::Char(c), KeyModifiers::NONE));
        }
        a.key(k(KeyCode::Enter, KeyModifiers::NONE));
        assert_eq!(a.view, ViewId::Dashboard);
        assert_eq!(a.dash.selected_key(), Some(&job));
        assert_eq!(a.transcript.len(), 3, "a /command does not add to the conversation");
    }

    /// Owner 2026-10-22: "click 1 or 2 or 3 and it doesn't change". On Home a number key opens
    /// that view while the prompt is empty and is typed once something is written; a click on
    /// the footer strip opens the view under it.
    #[test]
    fn number_keys_and_clicks_switch_views() {
        let k = |c: char| KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE);
        let mut a = test_app();
        a.key(k('2'));
        assert_eq!(a.view, ViewId::Dashboard);
        a.go(ViewId::Home);
        a.key(k('h'));
        a.key(k('3'));
        assert_eq!(a.view, ViewId::Home, "a digit after text is typed");
        assert_eq!(a.input.iter().collect::<String>(), "h3");
        a.input.clear();
        let (w, h) = (150u16, 58u16);
        let fy = frame_rows(h).1.unwrap();
        for (x0, x1, v) in strip_cells(&a, w) {
            a.go(ViewId::Home);
            a.mouse(MouseEvent { kind: MouseEventKind::Down(MouseButton::Left), column: (x0 + x1) / 2, row: fy, modifiers: KeyModifiers::NONE }, w, h);
            assert_eq!(a.view, v, "click at {x0}..{x1}");
        }
        assert_eq!(strip_cells(&a, w).len(), ViewId::TOP.len());
        // the Agents sections: 3 again, , and ., and a click on the bar
        a.go(ViewId::Home);
        a.key(k('3'));
        assert_eq!(a.view, ViewId::Agents);
        a.key(k('3'));
        assert_eq!(a.view, ViewId::Market);
        a.key(k('.'));
        assert_eq!(a.view, ViewId::Trade);
        a.key(k(','));
        assert_eq!(a.view, ViewId::Market);
        let body = body_rect(w, h);
        let (x0, x1, v) = section_cells(body)[3];
        a.mouse(MouseEvent { kind: MouseEventKind::Down(MouseButton::Left), column: (x0 + x1) / 2, row: body.y, modifiers: KeyModifiers::NONE }, w, h);
        assert_eq!((a.view, v), (ViewId::Boards, ViewId::Boards));
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
        // on Home a digit is typed once the prompt has text (with it empty, it switches views)
        a.key(k(KeyCode::Char('a'), KeyModifiers::NONE));
        a.key(k(KeyCode::Char('2'), KeyModifiers::NONE));
        assert_eq!((a.view, a.input.iter().collect::<String>()), (ViewId::Home, "a2".to_string()));
        a.key(k(KeyCode::Backspace, KeyModifiers::NONE));
        a.key(k(KeyCode::Backspace, KeyModifiers::NONE));
        a.key(k(KeyCode::Char('3'), KeyModifiers::ALT));
        assert_eq!(a.view, ViewId::Agents);
        a.key(k(KeyCode::Char('6'), KeyModifiers::NONE));
        assert_eq!(a.view, ViewId::Status);
        a.key(k(KeyCode::F(1), KeyModifiers::NONE));
        assert_eq!(a.view, ViewId::Home);
        // ctrl+p: views first, then commands; a command with args opens the Lookup input
        a.key(k(KeyCode::Char('p'), KeyModifiers::CONTROL));
        assert!(a.palette.is_some());
        assert_eq!(a.palette_items()[0].name(), "home");
        a.key(k(KeyCode::Char('t'), KeyModifiers::NONE));
        a.key(k(KeyCode::Char('x'), KeyModifiers::NONE));
        a.key(k(KeyCode::Enter, KeyModifiers::NONE));
        assert!(a.palette.is_none());
        assert_eq!(a.view, ViewId::Lookup);
        assert_eq!(a.lookup_input.iter().collect::<String>(), "/tx ");
        // the status command opens the Status view (inside the app, never the shell)
        a.palette = Some(Palette { filter: "status".into(), sel: 0 });
        a.key(k(KeyCode::Enter, KeyModifiers::NONE));
        assert_eq!(a.view, ViewId::Status);
        // a /line on Home that is not a view runs in the Lookup view
        a.key(k(KeyCode::F(1), KeyModifiers::NONE));
        for c in "/block 12".chars() {
            a.key(k(KeyCode::Char(c), KeyModifiers::NONE));
        }
        a.key(k(KeyCode::Enter, KeyModifiers::NONE));
        assert_eq!(a.view, ViewId::Lookup);
        assert_eq!(a.lookups.last().unwrap().query, "/block 12");
        // asking with no wallet and no API key: the setup line, never an answer
        a.key(k(KeyCode::F(1), KeyModifiers::NONE));
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
        // Esc never quits (it toggles views); ctrl+c and /quit do
        a.key(k(KeyCode::Esc, KeyModifiers::NONE));
        assert!(!a.quit);
        let mut b = test_app();
        b.key(k(KeyCode::Char('c'), KeyModifiers::CONTROL));
        assert!(b.quit);
        let mut c = test_app();
        for ch in "/quit".chars() {
            c.key(k(KeyCode::Char(ch), KeyModifiers::NONE));
        }
        c.key(k(KeyCode::Enter, KeyModifiers::NONE));
        assert!(c.quit);
    }

    #[test]
    fn dashboard_send_form_needs_a_key_and_a_confirm() {
        let k = |c: KeyCode| KeyEvent::new(c, KeyModifiers::NONE);
        let mut a = test_app();
        a.apply(Msg::Snap(Box::new(sample::snapshot())));
        a.go(ViewId::Dashboard);
        a.dash.tab = dash::Tab::Send;
        // no key: the form says so and digits switch views as usual
        let all = rows(&render(&a, 120, 40)).join("\n");
        assert!(all.contains(dash::NO_WALLET));
        assert!(!a.typing());
        a.has_key = true;
        assert!(a.typing());
        for c in "7xKX".chars() {
            a.key(k(KeyCode::Char(c)));
        }
        a.key(k(KeyCode::Enter));
        for c in "1.5".chars() {
            a.key(k(KeyCode::Char(c)));
        }
        assert_eq!(a.dash.send.amount.iter().collect::<String>(), "1.5");
        a.key(k(KeyCode::Enter));
        assert!(a.dash.send.confirm, "the first enter only asks");
        assert!(!a.dash.send.busy);
        a.key(k(KeyCode::Esc));
        assert!(!a.dash.send.confirm);
        // the half-typed form survives a trip to Home
        a.key(k(KeyCode::F(1)));
        a.key(k(KeyCode::F(2)));
        assert_eq!(a.dash.send.to.iter().collect::<String>(), "7xKX");
        assert_eq!(a.dash.send.amount.iter().collect::<String>(), "1.5");
    }

    pub(crate) fn test_app() -> App {
        let m = <Opts as clap::Args>::augment_args(clap::Command::new("t")).get_matches_from(["t", "--network", "localnet", "--rpc", "http://127.0.0.1:9", "--sol-rpc", "http://127.0.0.1:9", "--pool", "http://127.0.0.1:9"]);
        let opts = <Opts as clap::FromArgMatches>::from_arg_matches(&m).unwrap();
        let ctx = Arc::new(Ctx::new(opts.clone()).unwrap());
        let l = Launch {
            opts,
            forward: vec![],
            commands: vec![PaletteCmd { name: "status".into(), about: "Health of everything".into(), needs_args: false }, PaletteCmd { name: "tx".into(), about: "A tx".into(), needs_args: true }, PaletteCmd { name: "block".into(), about: "A block".into(), needs_args: true }],
            start: ViewId::Home,
        };
        let mut a = App::new(l, ctx, Net { api: None, gateway: None, key: None, api_key: None });
        a.network = "devnet";
        a.color = Mode::TrueColor;
        a.cwd = "~/cadre:main".into();
        a.agents = vec![AgentEntry { account: "A".into(), handle: "tide".into(), name: "Tide Clerk".into(), house: false, gh: None }];
        a.model = Some(Model { id: "llama-3.2-1b-instruct-w7a7".into(), server: "S".into() });
        a.dm_live = Some(false);
        a
    }
}
