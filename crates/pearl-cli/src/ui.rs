//! The dashboard: a retro blue terminal screen (header + block wordmark, red beta
//! banner, boxed panels with slash titles, a selectable table, a black shell strip
//! with a real command line and a hotkey bar). Rendering is a pure function of
//! [`App`], so ratatui's TestBackend renders it in tests and for screenshots.

use std::sync::mpsc;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{bail, Result};
use ratatui::crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph, Row, Table, TableState};
use ratatui::Frame;
use serde_json::Value;

use crate::brand::{self, TICKER};
use crate::commands::{self, Cmd};
use crate::ctx::Ctx;
use crate::fmt::{self, leader, short, tokens};
use crate::snapshot::{self, s, u, Snapshot};

pub const BLUE: Color = Color::Rgb(0, 0, 170);
pub const WHITE: Color = Color::Rgb(255, 255, 255);
pub const YELLOW: Color = Color::Rgb(255, 255, 85);
pub const RED: Color = Color::Rgb(255, 85, 85);
pub const GREEN: Color = Color::Rgb(85, 255, 85);
pub const CYAN: Color = Color::Rgb(85, 255, 255);
pub const BLACK: Color = Color::Rgb(0, 0, 0);
pub const DIM: Color = Color::Rgb(170, 170, 255);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tab {
    Jobs,
    Mining,
    Activity,
    Bridge,
    Agent,
}

impl Tab {
    pub const ALL: [Tab; 5] = [Tab::Jobs, Tab::Mining, Tab::Activity, Tab::Bridge, Tab::Agent];

    pub fn name(self) -> &'static str {
        match self {
            Tab::Jobs => "Jobs",
            Tab::Mining => "Mining",
            Tab::Activity => "Activity",
            Tab::Bridge => "Bridge",
            Tab::Agent => "Agent",
        }
    }

    pub fn parse(s: &str) -> Result<Tab> {
        Tab::ALL.into_iter().find(|t| t.name().eq_ignore_ascii_case(s)).map(Ok).unwrap_or_else(|| bail!("unknown tab {s:?} (jobs, mining, activity, bridge, agent)"))
    }
}

pub struct App {
    pub snap: Snapshot,
    pub tab: Tab,
    pub sel: usize,
    pub input: String,
    /// Last outputs, newest last: (is_error, text).
    pub log: Vec<(bool, String)>,
    pub detail: Option<(String, Vec<String>)>,
    pub busy: Option<String>,
    pub quit: bool,
    /// Has a key (writes possible), vs watching an address.
    pub connected: bool,
}

impl App {
    pub fn new(snap: Snapshot, connected: bool) -> Self {
        Self {
            snap,
            tab: Tab::Jobs,
            sel: 0,
            input: String::new(),
            log: vec![(false, "type `help` for commands; [1]-[5] switch the table, arrows + Enter open a row".into())],
            detail: None,
            busy: None,
            quit: false,
            connected,
        }
    }

    pub fn rows(&self) -> TableData {
        table_data(&self.snap, self.tab)
    }

    fn clamp_sel(&mut self) {
        let n = self.rows().rows.len();
        self.sel = if n == 0 { 0 } else { self.sel.min(n - 1) };
    }

    pub fn set_tab(&mut self, t: Tab) {
        if self.tab != t {
            self.tab = t;
            self.sel = 0;
        }
    }

    pub fn open_detail(&mut self) {
        let d = self.rows();
        if let Some(raw) = d.raw.get(self.sel) {
            self.detail = Some((format!("/{} {}", d.item, d.rows[self.sel][0].trim()), detail_lines(raw)));
        }
    }

    pub fn push_log(&mut self, err: bool, text: String) {
        for l in text.lines() {
            self.log.push((err, l.to_string()));
        }
        let n = self.log.len();
        if n > 200 {
            self.log.drain(..n - 200);
        }
    }
}

/// A table as display strings plus the JSON row behind each line.
pub struct TableData {
    pub item: &'static str,
    pub headers: Vec<&'static str>,
    pub widths: Vec<Constraint>,
    pub rows: Vec<Vec<String>>,
    pub raw: Vec<Value>,
    pub footer: String,
}

fn tok(units: u64) -> String {
    tokens(units as u128)
}

fn is_zero_hex(h: &str) -> bool {
    !h.is_empty() && h.chars().all(|c| c == '0')
}

pub fn table_data(sn: &Snapshot, tab: Tab) -> TableData {
    let me = sn.me.clone().unwrap_or_default();
    match tab {
        Tab::Jobs => {
            let raw: Vec<Value> = sn.jobs.iter().rev().cloned().collect();
            TableData {
                item: "Job",
                headers: vec!["NAME", "STATUS", "REWARD W", "FEE P", "SIZE", "BLOCKS", "DEADLINE"],
                widths: vec![Constraint::Length(15), Constraint::Length(10), Constraint::Length(14), Constraint::Length(12), Constraint::Length(14), Constraint::Length(7), Constraint::Min(8)],
                rows: raw
                    .iter()
                    .map(|j| {
                        vec![
                            format!("/ {}", short(s(j, "job_id"))),
                            s(j, "status").to_string(),
                            tok(u(j, "worker_pay")),
                            tok(u(j, "protocol_fee")),
                            format!("{}x{}x{}", u(j, "m"), u(j, "n"), u(j, "k")),
                            format!("{}/{}", u(j, "blocks_mined"), u(j, "max_blocks")),
                            u(j, "deadline_height").to_string(),
                        ]
                    })
                    .collect(),
                raw,
                footer: format!("{} jobs on chain", sn.jobs_total),
            }
        }
        Tab::Mining => {
            let (earned, mined) = sn.window_rewards();
            let pool = match &sn.pool {
                Some(p) => format!("pool {} miners", u(p, "miners")),
                None => "pool not connected".into(),
            };
            TableData {
                item: "Block",
                headers: vec!["HEIGHT", "MINER", "WORK", "REWARD", "TXS", "STATUS"],
                widths: vec![Constraint::Length(10), Constraint::Length(13), Constraint::Length(13), Constraint::Length(16), Constraint::Length(5), Constraint::Min(8)],
                rows: sn
                    .blocks
                    .iter()
                    .map(|b| {
                        let m = s(&b["header_fields"], "miner");
                        let job = s(&b["header_fields"], "job_id");
                        vec![
                            format!("/ {}", u(&b["header_fields"], "height")),
                            if m == me { "you".into() } else { short(m) },
                            if is_zero_hex(job) { "fallback".into() } else { format!("job {}", fmt::short_n(job, 3)) },
                            tok(u(b, "reward")),
                            b["txs"].as_array().map(|a| a.len()).unwrap_or(0).to_string(),
                            if b["final"] == true { "final".into() } else { "pending".into() },
                        ]
                    })
                    .collect(),
                raw: sn.blocks.clone(),
                footer: format!("yours {mined}/{} blocks, {} {TICKER} ({} pending)  {pool}", sn.blocks.len(), tok(earned), tok(sn.pending_rewards())),
            }
        }
        Tab::Activity => TableData {
            item: "Tx",
            headers: vec!["HEIGHT", "KIND", "ROLE", "FEE", "TX", "STATUS"],
            widths: vec![Constraint::Length(10), Constraint::Length(14), Constraint::Length(10), Constraint::Length(10), Constraint::Length(13), Constraint::Min(8)],
            rows: sn
                .txs
                .iter()
                .map(|t| {
                    vec![
                        format!("/ {}", u(t, "height")),
                        s(t, "kind").into(),
                        s(t, "role").into(),
                        tok(u(t, "fee")),
                        short(s(t, "tx_hash")),
                        if t["final"] == true { "final".into() } else { "pending".into() },
                    ]
                })
                .collect(),
            raw: sn.txs.clone(),
            footer: format!("{} txs touch this account", sn.txs.len()),
        },
        Tab::Bridge => {
            let raw: Vec<Value> = sn.withdrawals.iter().rev().cloned().collect();
            let rel = |n: u64| sn.sol.released.iter().find(|(k, _)| *k == n).map(|(_, r)| *r);
            TableData {
                item: "Withdrawal",
                headers: vec!["NONCE", "AMOUNT", "FROM", "TO (SOLANA)", "HEIGHT", "STATUS"],
                widths: vec![Constraint::Length(9), Constraint::Length(16), Constraint::Length(13), Constraint::Length(13), Constraint::Length(8), Constraint::Min(9)],
                rows: raw
                    .iter()
                    .map(|w| {
                        let n = u(w, "nonce");
                        vec![
                            format!("/ {n}"),
                            fmt::spl(u(w, "amount_spl") as u128),
                            short(s(w, "from")),
                            short(s(w, "solana_recipient")),
                            u(w, "height").to_string(),
                            match (rel(n), w["final"] == true) {
                                (Some(true), _) => "released".into(),
                                (_, false) => "locked".into(),
                                _ => "final".into(),
                            },
                        ]
                    })
                    .collect(),
                raw,
                footer: format!("{} withdrawals, {} deposits unlocked", sn.withdrawal_count, sn.deposit_count),
            }
        }
        Tab::Agent => {
            let mut rows = vec![];
            let mut raw = vec![];
            let mut add = |k: &str, v: String, r: Value| {
                rows.push(vec![format!("/ {k}"), v]);
                raw.push(r);
            };
            if sn.agent.is_object() {
                add("name", s(&sn.agent, "name").into(), sn.agent.clone());
                add("uri", s(&sn.agent, "uri").into(), sn.agent.clone());
                add("registered", format!("height {}", u(&sn.agent, "registered_height")), sn.agent.clone());
            } else {
                add("registration", "not registered (`register <name>`)".into(), Value::Null);
            }
            if sn.route.is_object() {
                let r = &sn.route;
                add("lien", if r["active"] == true { "active".into() } else { "ended".into() }, r.clone());
                add("route", format!("{}% to {}", u(r, "route_bps") as f64 / 100.0, short(s(r, "solana_recipient"))), r.clone());
                add("until", format!("height {}", u(r, "until_height")), r.clone());
                add("routed", format!("{} {TICKER}", tok(u(r, "routed"))), r.clone());
                add("withdrawn", format!("{} {TICKER}", tok(u(r, "withdrawn"))), r.clone());
                add("cap left", format!("{} {TICKER}", tok(u(r, "remaining_cap"))), r.clone());
            } else {
                add("lien", "none".into(), Value::Null);
            }
            add("agent credit", sn.credit.clone(), Value::Null);
            TableData {
                item: "Agent",
                headers: vec!["FIELD", "VALUE"],
                widths: vec![Constraint::Length(16), Constraint::Min(20)],
                rows,
                raw,
                footer: "payout lien = SetRoute (SPEC 4.4)".into(),
            }
        }
    }
}

/// Key: value lines of a JSON row for the detail popup.
pub fn detail_lines(v: &Value) -> Vec<String> {
    match v {
        Value::Object(o) => o
            .iter()
            .filter(|(k, _)| !matches!(k.as_str(), "tx" | "header" | "path" | "config" | "job_header" | "coinbase"))
            .flat_map(|(k, v)| match v {
                Value::Object(inner) => inner.iter().map(|(k2, v2)| format!("{k}.{k2:<14} {}", plain(v2))).collect::<Vec<_>>(),
                _ => vec![format!("{k:<18} {}", plain(v))],
            })
            .collect(),
        Value::Null => vec!["(nothing to open)".into()],
        o => vec![o.to_string()],
    }
}

fn plain(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Null => "-".into(),
        Value::Array(a) => format!("[{} items]", a.len()),
        o => o.to_string(),
    }
}

fn base() -> Style {
    Style::default().fg(WHITE).bg(BLUE)
}

fn panel(title: &str) -> Block<'static> {
    Block::default()
        .borders(Borders::ALL)
        .border_style(base())
        .style(base())
        .title(Line::from(Span::styled(format!("/{title}"), Style::default().fg(YELLOW).bg(BLUE))))
}

/// A two-part small panel: big value, a rule, then smaller lines.
fn small_panel(f: &mut Frame, area: Rect, title: &str, top: Vec<Span<'static>>, rest: Vec<Line<'static>>) {
    let b = panel(title);
    let inner = b.inner(area);
    f.render_widget(b, area);
    let mut lines = vec![Line::from(top), Line::from("\u{2500}".repeat(inner.width as usize))];
    lines.extend(rest);
    f.render_widget(Paragraph::new(lines).style(base()), inner);
}

fn v(s: String) -> Span<'static> {
    Span::styled(s, base().add_modifier(Modifier::BOLD))
}

fn lead(label: &str, value: String, w: u16, color: Color) -> Line<'static> {
    let t = leader(label, &value, w as usize);
    let cut = t.len() - value.len().min(t.len());
    Line::from(vec![Span::styled(t[..cut].to_string(), base()), Span::styled(t[cut..].to_string(), base().fg(color))])
}

pub fn render(f: &mut Frame, app: &App) {
    let area = f.area();
    f.render_widget(Block::default().style(base()), area);
    let sn = &app.snap;

    let mark = if area.height >= 36 { brand::wordmark(area.width.saturating_sub(2) as usize) } else { vec![] };
    let head_h = 2 + mark.len() as u16 + if mark.is_empty() { 0 } else { 1 };
    let [head, warn, body, shell] = Layout::vertical([Constraint::Length(head_h), Constraint::Length(3), Constraint::Min(8), Constraint::Length(5)]).areas(area);

    // header: two lines left, bracket mark right, wordmark under
    let mut hl = vec![
        Line::from(Span::styled(brand::NAME.to_string(), base().add_modifier(Modifier::BOLD))),
        Line::from(Span::styled(brand::TAGLINE.to_string(), base())),
    ];
    if !mark.is_empty() {
        hl.push(Line::from(""));
        hl.extend(mark.iter().map(|l| Line::from(Span::styled(l.clone(), base()))));
    }
    f.render_widget(Paragraph::new(hl).style(base()), head);
    let bm = brand::bracket_mark();
    let bw = bm.chars().count() as u16;
    if head.width > bw + 2 {
        f.render_widget(Paragraph::new(bm).style(base().add_modifier(Modifier::BOLD)), Rect { x: head.x + head.width - bw - 1, y: head.y, width: bw, height: 1 });
    }

    // red beta banner
    let stars = "*".repeat(warn.width as usize);
    let red = base().fg(RED);
    f.render_widget(Paragraph::new(vec![Line::from(Span::styled(stars.clone(), red)), Line::from(Span::styled(brand::WARNING, red)), Line::from(Span::styled(stars, red))]).style(base()), warn);

    let [left, right] = Layout::horizontal([Constraint::Percentage(68), Constraint::Percentage(32)]).areas(body);
    let [tops, table] = Layout::vertical([Constraint::Length(6), Constraint::Min(5)]).areas(left);
    let [p1, p2, p3] = Layout::horizontal([Constraint::Ratio(1, 3), Constraint::Ratio(1, 3), Constraint::Ratio(1, 3)]).areas(tops);
    render_small_panels(f, sn, [p1, p2, p3]);
    render_table(f, app, table);
    let [br, ag] = Layout::vertical([Constraint::Length(12), Constraint::Min(6)]).areas(right);
    render_bridge(f, sn, br);
    render_agent(f, sn, ag);
    render_shell(f, app, shell);

    if let Some((title, lines)) = &app.detail {
        let w = (area.width * 3 / 4).max(40).min(area.width);
        let h = (lines.len() as u16 + 3).min(area.height.saturating_sub(4)).max(5);
        let r = Rect { x: area.x + (area.width - w) / 2, y: area.y + (area.height.saturating_sub(h)) / 2, width: w, height: h };
        f.render_widget(Clear, r);
        let b = panel(title.trim_start_matches('/')).title_bottom(Line::from(vec![Span::styled("Close(); Tap Esc or [", base()), Span::styled("esc", Style::default().fg(BLACK).bg(CYAN)), Span::styled("]", base())]));
        let inner = b.inner(r);
        f.render_widget(b, r);
        f.render_widget(Paragraph::new(lines.iter().map(|l| Line::from(l.clone())).collect::<Vec<_>>()).style(base()), inner);
    }
}

fn render_small_panels(f: &mut Frame, sn: &Snapshot, [p1, p2, p3]: [Rect; 3]) {
    // /Wallet Balance
    let bal = match sn.balance() {
        Some(b) => format!("{} {TICKER}", tok(b)),
        None if sn.me.is_none() => "no wallet".into(),
        None => "-".into(),
    };
    let mut rest = vec![Line::from(format!("{} Pending {TICKER}", tok(sn.pending_rewards())))];
    rest.push(Line::from(match sn.sol.my_spl {
        Some(x) => format!("{} {TICKER} on Solana", fmt::compact(x as u128, fmt::SPL_DP)),
        None => "Solana: no key".into(),
    }));
    small_panel(f, p1, "Wallet Balance", vec![v(bal)], rest);

    // /Market: no price source; storefront terms and vault only
    let store = sn.sol.store.as_ref();
    let top = match store {
        Some(c) => format!("job from {} SOL", fmt::sol(c.min_sol_in as u128)),
        None => "storefront n/a".into(),
    };
    let fill = match store {
        Some(c) if c.total_sol > 0 => format!("{} orders @ {} {TICKER}/SOL", c.orders, fmt::compact((c.total_tokens as u128) * 1_000_000_000 / c.total_sol as u128, fmt::SPL_DP)),
        Some(c) => format!("{} orders", c.orders),
        None => "-".into(),
    };
    small_panel(f, p2, "Market", vec![v(top)], vec![Line::from(fill), Line::from(Span::styled("no price source", base().fg(DIM)))]);

    // /Protocol
    let top = if sn.chain_ok() { format!("height {}", sn.height()) } else { "chain offline".into() };
    let sub = sn.job_subsidy().map(|x| format!("job blk up to {} {TICKER}", fmt::compact(x as u128, fmt::CHAIN_DP))).unwrap_or("-".into());
    small_panel(
        f,
        p3,
        "Protocol",
        vec![v(top)],
        vec![Line::from(format!("final {}  K {}  0x{:08x}", u(&sn.info, "finalized_height"), sn.finality_k(), u(&sn.info, "target_bits"))), Line::from(sub)],
    );
}

fn render_table(f: &mut Frame, app: &App, area: Rect) {
    let d = app.rows();
    let footer = Line::from(vec![
        Span::styled(format!("{}(); Tap Enter or [", d.item), base()),
        Span::styled("open", Style::default().fg(BLACK).bg(CYAN)),
        Span::styled(format!("]  {}", d.footer), base()),
    ]);
    let b = panel(app.tab.name()).title_bottom(footer);
    let header = Row::new(d.headers.iter().map(|h| h.to_string())).style(base().fg(YELLOW));
    let rows: Vec<Row> = d.rows.iter().map(|r| Row::new(r.clone()).style(base())).collect();
    let empty = rows.is_empty();
    let t = Table::new(rows, d.widths.clone()).header(header).block(b).style(base()).row_highlight_style(Style::default().fg(BLACK).bg(GREEN));
    let mut st = TableState::default();
    if !empty {
        st.select(Some(app.sel));
    }
    f.render_stateful_widget(t, area, &mut st);
    if empty {
        let msg = match app.tab {
            Tab::Jobs => "no jobs yet (`job post --pay W --fee P`)",
            Tab::Activity if app.snap.me.is_none() => "no wallet: pass --key FILE",
            Tab::Activity => "no transactions yet",
            Tab::Bridge => "no withdrawals yet (`bridge out <amount>`)",
            _ => "nothing here yet",
        };
        f.render_widget(Paragraph::new(format!("  {msg}")).style(base().fg(DIM)), Rect { x: area.x + 1, y: area.y + 2, width: area.width.saturating_sub(2), height: 1 });
    }
}

fn render_bridge(f: &mut Frame, sn: &Snapshot, area: Rect) {
    let b = panel("Bridge");
    let inner = b.inner(area);
    f.render_widget(b, area);
    let w = inner.width;
    let supply = u(&sn.escrow, "total_supply") as u128;
    let outside = u(&sn.escrow, "outside_escrow") as u128;
    let pct = if supply > 0 { format!("{:.2}%", outside as f64 * 100.0 / supply as f64) } else { "-".into() };
    let inv = sn.invariant();
    let mut lines = vec![
        lead("Escrow", fmt::compact(u(&sn.escrow, "escrow_balance") as u128, fmt::CHAIN_DP), w, WHITE),
        lead("Supply bridged", pct, w, WHITE),
        lead("Vault SPL", sn.sol.vault_spl.map(|x| fmt::compact(x as u128, fmt::SPL_DP)).unwrap_or("n/a".into()), w, WHITE),
        lead("Mining pool", fmt::compact(u(&sn.escrow, "pool_balance") as u128, fmt::CHAIN_DP), w, WHITE),
        lead("Deposits in", format!("{} (+{} in flight)", sn.deposit_count, sn.sol.inflight_dep_count), w, WHITE),
        lead("Withdrawals", format!("{} ({} unreleased)", sn.withdrawal_count, sn.sol.released.iter().filter(|(_, r)| !r).count()), w, WHITE),
        Line::from(Span::styled("vault SPL x 1000 == 1e18 - escrow", base().fg(DIM))),
    ];
    lines.push(match inv {
        Some(i) if i.ok && i.strict => lead("Invariant", "OK".into(), w, GREEN),
        Some(i) if i.ok => lead("Invariant", "OK (in flight)".into(), w, GREEN),
        Some(_) => lead("Invariant", "BROKEN".into(), w, RED),
        None => lead("Invariant", "n/a".into(), w, DIM),
    });
    let modes = sn.sol.bridge.as_ref().map(|c| match c.release_modes {
        1 => "signers",
        2 => "proof",
        3 => "signers+proof",
        _ => "off",
    });
    lines.push(lead("Release path", modes.unwrap_or("n/a").into(), w, WHITE));
    f.render_widget(Paragraph::new(lines).style(base()), inner);
}

fn render_agent(f: &mut Frame, sn: &Snapshot, area: Rect) {
    let b = panel("Agent");
    let inner = b.inner(area);
    f.render_widget(b, area);
    let w = inner.width;
    let reg = if sn.agent.is_object() { s(&sn.agent, "name").to_string() } else if sn.me.is_some() { "not registered".into() } else { "no wallet".into() };
    let lien = if sn.route.is_object() {
        if sn.route["active"] == true {
            format!("{}% to {}", u(&sn.route, "route_bps") as f64 / 100.0, short(s(&sn.route, "solana_recipient")))
        } else {
            "ended".into()
        }
    } else {
        "none".into()
    };
    let credit_color = if sn.credit == "reachable" { GREEN } else { DIM };
    let mut lines = vec![
        lead("Registered", reg, w, WHITE),
        lead("Payout lien", lien, w, WHITE),
        lead("Agent Credit", fmt::short_n(&sn.credit, 6), w, credit_color),
        lead("Pool", if sn.pool.is_some() { "connected".into() } else { "not connected".into() }, w, if sn.pool.is_some() { GREEN } else { DIM }),
        lead("Miner", sn.miner_pid.map(|p| format!("running pid {p}")).unwrap_or("stopped".into()), w, if sn.miner_pid.is_some() { GREEN } else { WHITE }),
        lead("Peers", u(&sn.info, "peers").to_string(), w, WHITE),
    ];
    if !sn.errors.is_empty() {
        lines.push(Line::from(Span::styled(format!("! {}", sn.errors[0]), base().fg(RED))));
    }
    f.render_widget(Paragraph::new(lines).style(base()), inner);
}

fn render_shell(f: &mut Frame, app: &App, area: Rect) {
    let sh = Style::default().fg(WHITE).bg(BLACK);
    let sn = &app.snap;
    let mut lines = vec![];
    // last two output lines
    let last: Vec<&(bool, String)> = app.log.iter().rev().take(2).collect();
    for (err, t) in last.into_iter().rev() {
        lines.push(Line::from(Span::styled(t.clone(), sh.fg(if *err { RED } else { Color::Rgb(200, 200, 200) }))));
    }
    while lines.len() < 2 {
        lines.insert(0, Line::from(""));
    }
    let (who, addr) = match (&sn.me, app.connected) {
        (Some(a), true) => ("Wallet Connected_ ", short(a)),
        (Some(a), false) => ("Watching_ ", short(a)),
        (None, _) => ("No wallet_ ", "--key FILE".into()),
    };
    lines.push(Line::from(vec![
        Span::styled(format!("$ {who}"), sh),
        Span::styled(addr, sh.fg(GREEN)),
        Span::styled(" ", sh),
        Span::styled(format!("~/{}", app.tab.name()), sh.fg(CYAN)),
    ]));
    let mut prompt = vec![Span::styled("$ ", sh), Span::styled(app.input.clone(), sh)];
    match &app.busy {
        Some(b) => prompt.push(Span::styled(format!(" [{b}...]"), sh.fg(YELLOW))),
        None => prompt.push(Span::styled(" ", Style::default().bg(GREEN))),
    }
    lines.push(Line::from(prompt));
    let keys = [("1", "Jobs"), ("2", "Mining"), ("3", "Activity"), ("4", "Bridge"), ("5", "Agent"), ("6", "Send"), ("0", "Quit")];
    let mut ks = vec![];
    for (k, label) in keys {
        ks.push(Span::styled(format!("[{k}]"), sh));
        let on = Tab::ALL.iter().any(|t| t.name() == label && *t == app.tab);
        ks.push(Span::styled(label, Style::default().fg(BLACK).bg(if on { GREEN } else { CYAN })));
        ks.push(Span::styled(" ", sh));
    }
    lines.push(Line::from(ks));
    f.render_widget(Paragraph::new(lines).style(sh), area);
}

enum Msg {
    Snap(Box<Snapshot>),
    Done(String, Result<commands::Out, String>),
}

/// Handle one key. Returns a command line to run, if Enter submitted one.
pub fn on_key(app: &mut App, code: KeyCode, mods: KeyModifiers) -> Option<String> {
    if mods.contains(KeyModifiers::CONTROL) && matches!(code, KeyCode::Char('c') | KeyCode::Char('d')) {
        app.quit = true;
        return None;
    }
    if app.detail.is_some() {
        if matches!(code, KeyCode::Esc | KeyCode::Enter | KeyCode::Backspace | KeyCode::Char('q')) {
            app.detail = None;
        }
        return None;
    }
    let n = app.rows().rows.len();
    match code {
        KeyCode::Esc => app.input.clear(),
        KeyCode::Enter if app.input.trim().is_empty() => app.open_detail(),
        KeyCode::Enter => return Some(std::mem::take(&mut app.input)),
        KeyCode::Backspace => {
            app.input.pop();
        }
        KeyCode::Up => app.sel = app.sel.saturating_sub(1),
        KeyCode::Down if n > 0 => app.sel = (app.sel + 1).min(n - 1),
        KeyCode::PageUp => app.sel = app.sel.saturating_sub(10),
        KeyCode::PageDown if n > 0 => app.sel = (app.sel + 10).min(n - 1),
        KeyCode::Home => app.sel = 0,
        KeyCode::End if n > 0 => app.sel = n - 1,
        KeyCode::Tab => {
            let i = Tab::ALL.iter().position(|t| *t == app.tab).unwrap_or(0);
            app.set_tab(Tab::ALL[(i + 1) % Tab::ALL.len()]);
        }
        KeyCode::Char(c) if app.input.is_empty() && c.is_ascii_digit() => match c {
            '1'..='5' => app.set_tab(Tab::ALL[(c as u8 - b'1') as usize]),
            '6' => app.input = "send ".into(),
            '0' => app.quit = true,
            _ => {}
        },
        KeyCode::Char(c) => app.input.push(c),
        _ => {}
    }
    None
}

pub fn run(ctx: Ctx) -> Result<()> {
    let ctx = Arc::new(ctx);
    let first = snapshot::gather(&ctx);
    let mut app = App::new(first, ctx.has_key());
    let (tx, rx) = mpsc::channel::<Msg>();
    let (kick_tx, kick_rx) = mpsc::channel::<()>();
    {
        let (ctx, tx) = (ctx.clone(), tx.clone());
        std::thread::spawn(move || loop {
            // refresh every 2 s, or at once after a command
            let _ = kick_rx.recv_timeout(Duration::from_secs(2));
            while kick_rx.try_recv().is_ok() {}
            if tx.send(Msg::Snap(Box::new(snapshot::gather(&ctx)))).is_err() {
                return;
            }
        });
    }
    let mut term = ratatui::init();
    let res = (|| -> Result<()> {
        while !app.quit {
            term.draw(|f| render(f, &app))?;
            while let Ok(m) = rx.try_recv() {
                match m {
                    Msg::Snap(s) => {
                        app.snap = *s;
                        app.clamp_sel();
                    }
                    Msg::Done(line, r) => {
                        app.busy = None;
                        match r {
                            Ok(o) => app.push_log(false, o.text),
                            Err(e) => app.push_log(true, format!("{line}: {e}")),
                        }
                        let _ = kick_tx.send(());
                    }
                }
            }
            if !event::poll(Duration::from_millis(100))? {
                continue;
            }
            let Event::Key(k) = event::read()? else { continue };
            if k.kind != KeyEventKind::Press {
                continue;
            }
            let Some(line) = on_key(&mut app, k.code, k.modifiers) else { continue };
            match commands::parse_line(&line) {
                Err(e) => app.push_log(true, e.to_string()),
                Ok(Cmd::Quit) => app.quit = true,
                Ok(Cmd::Dashboard) => app.push_log(true, "already in the dashboard".into()),
                Ok(Cmd::Help) => app.detail = Some(("Help".into(), commands::HELP.lines().map(String::from).collect())),
                Ok(_) if app.busy.is_some() => app.push_log(true, "a command is still running".into()),
                Ok(cmd) => {
                    app.push_log(false, format!("$ {line}"));
                    app.busy = Some(line.split_whitespace().next().unwrap_or("").to_string());
                    let (ctx, tx) = (ctx.clone(), tx.clone());
                    std::thread::spawn(move || {
                        let r = commands::exec(&ctx, &cmd, true).map_err(|e| format!("{e:#}"));
                        let _ = tx.send(Msg::Done(line, r));
                    });
                }
            }
        }
        Ok(())
    })();
    ratatui::restore();
    res
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::snapshot::sample;
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;

    fn draw(app: &App, w: u16, h: u16) -> ratatui::buffer::Buffer {
        let mut t = Terminal::new(TestBackend::new(w, h)).unwrap();
        t.draw(|f| render(f, app)).unwrap();
        t.backend().buffer().clone()
    }

    fn text(b: &ratatui::buffer::Buffer) -> Vec<String> {
        (0..b.area.height).map(|y| (0..b.area.width).map(|x| b[(x, y)].symbol().to_string()).collect::<String>()).collect()
    }

    fn find(lines: &[String], needle: &str) -> Option<(u16, u16)> {
        lines.iter().enumerate().find_map(|(y, l)| l.find(needle).map(|i| (l[..i].chars().count() as u16, y as u16)))
    }

    #[test]
    fn main_screen_renders_reference_layout() {
        let app = App::new(sample::snapshot(), true);
        let b = draw(&app, 140, 46);
        let t = text(&b);
        let all = t.join("\n");
        // header, wordmark from brand::NAME, bracket mark, red banner
        assert!(t[0].starts_with(brand::NAME));
        assert!(t[0].trim_end().ends_with(&brand::bracket_mark()));
        let wm = brand::wordmark(138);
        assert!(all.contains(&wm[0]), "wordmark row 0 missing");
        let (wx, wy) = find(&t, brand::WARNING).unwrap();
        assert_eq!(b[(wx, wy)].fg, RED);
        assert!(t[wy as usize - 1].chars().all(|c| c == '*'));
        // panel titles in yellow, embedded in the top border
        for title in ["/Wallet Balance", "/Market", "/Protocol", "/Jobs", "/Bridge", "/Agent"] {
            let (x, y) = find(&t, title).unwrap_or_else(|| panic!("{title} missing"));
            assert_eq!(b[(x, y)].fg, YELLOW, "{title}");
            assert_eq!(b[(x - 1, y)].symbol(), "\u{250c}", "{title} sits on the border");
        }
        // values traced from the sample snapshot
        assert!(all.contains("40.74 LODE"));
        assert!(all.contains("2 Pending LODE"));
        assert!(all.contains("8.69M LODE on Solana"));
        assert!(all.contains("job from 0.01 SOL"));
        assert!(all.contains("no price source"));
        assert!(all.contains("height 260"));
        assert!(all.contains("Invariant") && all.contains("OK (in flight)"));
        // table: yellow header, '/ ' rows, first row selected in green
        let (hx, hy) = find(&t, "NAME").unwrap();
        assert_eq!(b[(hx, hy)].fg, YELLOW);
        let (rx, ry) = find(&t, "/ bbbb...bbbb").unwrap();
        assert_eq!(b[(rx, ry)].bg, GREEN);
        assert_eq!(b[(rx, ry)].fg, BLACK);
        assert!(find(&t, "/ aaaa...aaaa").is_some());
        assert!(all.contains("Job(); Tap Enter or [open]"));
        let (ox, oy) = find(&t, "open]").unwrap();
        assert_eq!(b[(ox, oy)].bg, CYAN);
        // shell strip: black, address green, path cyan, hotkeys cyan
        let (cx, cy) = find(&t, "Wallet Connected_").unwrap();
        assert_eq!(b[(cx, cy)].bg, BLACK);
        let (ax, ay) = find(&t, &short(sample::ME)).unwrap();
        assert_eq!(b[(ax, ay)].fg, GREEN);
        let (px, py) = find(&t, "~/Jobs").unwrap();
        assert_eq!(b[(px, py)].fg, CYAN);
        let (kx, ky) = find(&t, "[2]Mining").unwrap();
        assert_eq!(b[(kx + 3, ky)].bg, CYAN);
        // the whole body is blue
        assert_eq!(b[(70, 20)].bg, BLUE);
        // no em dash anywhere on screen
        assert!(!all.contains('\u{2014}'));
    }

    #[test]
    fn tabs_selection_detail_and_input() {
        let mut app = App::new(sample::snapshot(), true);
        on_key(&mut app, KeyCode::Down, KeyModifiers::NONE);
        assert_eq!(app.sel, 1);
        on_key(&mut app, KeyCode::Down, KeyModifiers::NONE);
        assert_eq!(app.sel, 1, "clamped to the last job");
        on_key(&mut app, KeyCode::Enter, KeyModifiers::NONE);
        let (title, lines) = app.detail.clone().unwrap();
        assert_eq!(title, "/Job / aaaa...aaaa");
        assert!(lines.iter().any(|l| l.starts_with("status") && l.ends_with("completed")));
        let t = text(&draw(&app, 140, 46)).join("\n");
        assert!(t.contains("Close(); Tap Esc or [esc]"));
        on_key(&mut app, KeyCode::Esc, KeyModifiers::NONE);
        assert!(app.detail.is_none());

        on_key(&mut app, KeyCode::Char('2'), KeyModifiers::NONE);
        assert_eq!((app.tab, app.sel), (Tab::Mining, 0));
        let t = text(&draw(&app, 140, 46));
        assert!(find(&t, "/Mining").is_some());
        assert!(t.join("\n").contains("yours 3/8 blocks, 3 LODE (2 pending)"));
        assert!(t.join("\n").contains("fallback"));
        on_key(&mut app, KeyCode::Char('4'), KeyModifiers::NONE);
        assert!(text(&draw(&app, 140, 46)).join("\n").contains("released"));
        on_key(&mut app, KeyCode::Char('5'), KeyModifiers::NONE);
        assert!(text(&draw(&app, 140, 46)).join("\n").contains("50% to 7xKX...gAsU"));

        // digits are hotkeys only on an empty line
        on_key(&mut app, KeyCode::Char('6'), KeyModifiers::NONE);
        assert_eq!(app.input, "send ");
        for c in "abc 15".chars() {
            on_key(&mut app, KeyCode::Char(c), KeyModifiers::NONE);
        }
        assert_eq!(app.tab, Tab::Agent);
        let t = text(&draw(&app, 140, 46)).join("\n");
        assert!(t.contains("$ send abc 15"));
        assert_eq!(on_key(&mut app, KeyCode::Enter, KeyModifiers::NONE).as_deref(), Some("send abc 15"));
        assert!(app.input.is_empty());
        on_key(&mut app, KeyCode::Char('0'), KeyModifiers::NONE);
        assert!(app.quit);
    }

    #[test]
    fn small_and_offline_screens_render() {
        // narrow terminal: wordmark falls back or drops, nothing panics
        let app = App::new(sample::snapshot(), false);
        let t = text(&draw(&app, 80, 30)).join("\n");
        assert!(t.contains("Watching_"));
        assert!(!t.contains("$$"));
        let mut off = Snapshot::default();
        off.errors.push("agent chain: connection refused".into());
        off.credit = "not connected".into();
        let t = text(&draw(&App::new(off, false), 120, 40)).join("\n");
        assert!(t.contains("chain offline"));
        assert!(t.contains("No wallet_"));
        assert!(t.contains("no jobs yet"));
        assert!(t.contains("Invariant") && t.contains("n/a"));
    }
}
