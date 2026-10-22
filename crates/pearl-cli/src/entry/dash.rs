//! The Dashboard view, in the entry screen's style: quiet cards (blue bar, `#1e1e1e`
//! panel, gray labels, white values) for the wallet, market, protocol and mining; a
//! tabbed table (jobs, blocks, activity, withdrawals, agent fields, send) on the
//! terminal's own background whose selected row is the panel color with the blue bar;
//! the bridge and agent cards on the right. Every figure is a field of the poller's
//! [`Snapshot`] (RPC reads, or arithmetic over them named in snapshot.rs); with nothing
//! to show a card says what is missing.
//!
//! The mapping from a snapshot to what is on screen ([`wallet`], [`market`], [`protocol`],
//! [`mining`], [`bridge`], [`agent`], [`table`]) is pure, so tests check it field by field.

use std::cell::Cell;
use std::collections::HashMap;

use anyhow::{bail, Result};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Modifier;
use serde_json::Value;

use super::draw::{self, card, clip, kv, put, put_right, selected_row, str_w, Pen};
use super::theme::{BLUE, DIM, GREEN, MUTED, PANEL, RED, TEXT, YELLOW};
use crate::brand::TICKER;
use crate::fmt::{self, short, tokens};
use crate::snapshot::{s, u, Snapshot};
use crate::term::status_tone;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Tab {
    Jobs,
    Mining,
    Activity,
    Bridge,
    Agent,
    Send,
}

impl Tab {
    pub const ALL: [Tab; 6] = [Tab::Jobs, Tab::Mining, Tab::Activity, Tab::Bridge, Tab::Agent, Tab::Send];

    pub fn name(self) -> &'static str {
        match self {
            Tab::Jobs => "Jobs",
            Tab::Mining => "Mining",
            Tab::Activity => "Activity",
            Tab::Bridge => "Bridge",
            Tab::Agent => "Agent",
            Tab::Send => "Send",
        }
    }

    pub fn parse(s: &str) -> Result<Tab> {
        match Tab::ALL.into_iter().find(|t| t.name().eq_ignore_ascii_case(s)) {
            Some(t) => Ok(t),
            None => bail!("unknown tab {s:?} (jobs, mining, activity, bridge, agent, send)"),
        }
    }

    pub fn next(self, back: bool) -> Tab {
        let i = Tab::ALL.iter().position(|t| *t == self).unwrap_or(0);
        let n = Tab::ALL.len();
        Tab::ALL[if back { (i + n - 1) % n } else { (i + 1) % n }]
    }
}

// ---------------------------------------------------------------------------------------
// data mapping (pure)

/// One line of a card: label, value, value color.
pub type Line = (String, String, u32);

/// A card: its title, a headline (text, color) and label/value lines.
#[derive(Debug, Clone, PartialEq)]
pub struct Card {
    pub title: String,
    pub head: (String, u32),
    pub lines: Vec<Line>,
}

fn tok(units: u64) -> String {
    tokens(units as u128)
}

/// Chain units for the narrow cards: two decimals under 10K (`776.83`), else compact (`12.39M`).
fn cmp(units: u64) -> String {
    if units < 10_000 * 1_000_000_000 {
        fmt::amount(units as u128, fmt::CHAIN_DP, 2)
    } else {
        fmt::compact(units as u128, fmt::CHAIN_DP)
    }
}

fn l(label: &str, value: impl Into<String>, c: u32) -> Line {
    (label.to_string(), value.into(), c)
}

pub const NO_WALLET: &str = "no wallet: cadre wallet use --key FILE";

pub fn wallet(sn: &Snapshot) -> Card {
    let title = match &sn.me {
        Some(a) => format!("Wallet  {}", fmt::short_addr(a)),
        None => "Wallet".into(),
    };
    let Some(_) = &sn.me else {
        return Card { title, head: (NO_WALLET.into(), MUTED), lines: vec![l("watch one", "cadre wallet use --address ADDR", DIM)] };
    };
    let head = match sn.balance() {
        Some(b) => (format!("{} {TICKER}", tok(b)), TEXT),
        None if !sn.chain_ok() => ("chain unavailable".into(), MUTED),
        None => ("balance unavailable".into(), MUTED),
    };
    let mut lines = vec![l("pending", format!("{} {TICKER}", cmp(sn.pending_rewards())), TEXT)];
    lines.push(match sn.sol.my_spl {
        Some(x) => l("on Solana", format!("{} {TICKER}", fmt::compact(x as u128, fmt::SPL_DP)), TEXT),
        None if sn.sol_me.is_none() => l("on Solana", "no Solana key", MUTED),
        None => l("on Solana", "unavailable", MUTED),
    });
    lines.push(match sn.sol.my_sol {
        Some(x) => l("SOL", fmt::sol(x as u128), TEXT),
        None if sn.sol_me.is_none() => l("SOL", "no Solana key", MUTED),
        None => l("SOL", "unavailable", MUTED),
    });
    Card { title, head, lines }
}

pub fn market(sn: &Snapshot) -> Card {
    let title = "Market".to_string();
    match sn.sol.store.as_ref() {
        Some(c) => {
            let fill = if c.total_sol > 0 {
                l("CADRE per SOL", fmt::compact((c.total_tokens as u128) * 1_000_000_000 / c.total_sol as u128, fmt::SPL_DP), TEXT)
            } else {
                l("CADRE per SOL", "no orders yet", MUTED)
            };
            Card {
                title,
                head: (format!("job from {} SOL", fmt::sol(c.min_sol_in as u128)), TEXT),
                lines: vec![l("orders", c.orders.to_string(), TEXT), fill, l("price", "no price source", DIM)],
            }
        }
        None => {
            let why = sn.sol.error.clone().unwrap_or_else(|| "storefront not initialised on this cluster".into());
            Card { title, head: ("storefront unavailable".into(), MUTED), lines: vec![l("", why, DIM)] }
        }
    }
}

pub fn protocol(sn: &Snapshot) -> Card {
    let title = "Protocol".to_string();
    if !sn.chain_ok() {
        let why = sn.errors.first().cloned().unwrap_or_else(|| "no answer from the node".into());
        return Card { title, head: ("chain offline".into(), RED), lines: vec![l("", why, DIM)] };
    }
    let sub = sn.job_subsidy().map(|x| format!("{} {TICKER}", cmp(x))).unwrap_or("unavailable".into());
    Card {
        title,
        head: (format!("block {}", sn.height()), TEXT),
        lines: vec![
            l("final", format!("{}  K {}", u(&sn.info, "finalized_height"), sn.finality_k()), TEXT),
            l("difficulty", format!("0x{:08x}", u(&sn.info, "target_bits")), TEXT),
            l("max subsidy", sub, TEXT),
            l("peers", u(&sn.info, "peers").to_string(), TEXT),
        ],
    }
}

pub fn mining(sn: &Snapshot) -> Card {
    let title = "Mining".to_string();
    let (earned, mined) = sn.window_rewards();
    let pool = match &sn.pool {
        Some(p) => l("pool", format!("{} miners", u(p, "miners")), TEXT),
        None => l("pool", "not connected", MUTED),
    };
    let miner = match sn.miner_pid {
        Some(p) => l("miner", format!("running, pid {p}"), GREEN),
        None => l("miner", "not started here", MUTED),
    };
    if sn.me.is_none() {
        return Card { title, head: (format!("{} recent blocks", sn.blocks.len()), TEXT), lines: vec![l("yours", NO_WALLET, MUTED), pool, miner] };
    }
    Card {
        title,
        head: (format!("{mined} of {} recent blocks", sn.blocks.len()), TEXT),
        lines: vec![l("earned", format!("{} {TICKER}", cmp(earned)), TEXT), l("pending", format!("{} {TICKER}", cmp(sn.pending_rewards())), TEXT), pool, miner],
    }
}

pub fn bridge(sn: &Snapshot) -> Card {
    let supply = u(&sn.escrow, "total_supply") as u128;
    let outside = u(&sn.escrow, "outside_escrow") as u128;
    let have = sn.escrow.is_object();
    let na = |c: bool, v: String| if c { (v, TEXT) } else { ("unavailable".to_string(), MUTED) };
    let (esc, ec) = na(have, format!("{} {TICKER}", fmt::compact(u(&sn.escrow, "escrow_balance") as u128, fmt::CHAIN_DP)));
    let (pct, pc) = na(have && supply > 0, format!("{:.2}%", outside as f64 * 100.0 / supply.max(1) as f64));
    let (pool, poc) = na(have, format!("{} {TICKER}", fmt::compact(u(&sn.escrow, "pool_balance") as u128, fmt::CHAIN_DP)));
    let vault = match sn.sol.vault_spl {
        Some(x) => (format!("{} SPL", fmt::compact(x as u128, fmt::SPL_DP)), TEXT),
        None => ("unavailable".into(), MUTED),
    };
    let inv = match sn.invariant() {
        Some(i) if i.ok && i.strict => ("OK".to_string(), GREEN),
        Some(i) if i.ok => ("OK (in flight)".to_string(), GREEN),
        Some(_) => ("BROKEN".to_string(), RED),
        None => ("unavailable".to_string(), MUTED),
    };
    let modes = sn.sol.bridge.as_ref().map(|c| match c.release_modes {
        1 => "signers",
        2 => "proof",
        3 => "signers + proof",
        _ => "off",
    });
    let unreleased = sn.sol.released.iter().filter(|(_, r)| !r).count();
    Card {
        title: "Bridge".into(),
        head: (esc, ec),
        lines: vec![
            l("supply bridged", pct, pc),
            l("vault", vault.0, vault.1),
            l("mining pool", pool, poc),
            l("deposits", format!("{} (+{} in flight)", sn.deposit_count, sn.sol.inflight_dep_count), if have { TEXT } else { MUTED }),
            l("withdrawals", format!("{} ({unreleased} unreleased)", sn.withdrawal_count), if have { TEXT } else { MUTED }),
            l("invariant", inv.0, inv.1),
            l("release path", modes.unwrap_or("unavailable"), if modes.is_some() { TEXT } else { MUTED }),
        ],
    }
}

pub fn agent(sn: &Snapshot) -> Card {
    let reg = if sn.agent.is_object() {
        (s(&sn.agent, "name").to_string(), TEXT)
    } else if sn.me.is_some() {
        ("not registered".to_string(), MUTED)
    } else {
        (NO_WALLET.to_string(), MUTED)
    };
    let lien = if sn.route.is_object() {
        if sn.route["active"] == true {
            (format!("{}% to {}", u(&sn.route, "route_bps") as f64 / 100.0, short(s(&sn.route, "solana_recipient"))), TEXT)
        } else {
            ("ended".to_string(), MUTED)
        }
    } else {
        ("none".to_string(), MUTED)
    };
    let credit = (sn.credit.clone(), if sn.credit == "reachable" { GREEN } else { MUTED });
    Card {
        title: "Agent".into(),
        head: reg,
        lines: vec![
            l("payout lien", lien.0, lien.1),
            l("agent credit", credit.0, credit.1),
            l("pool", if sn.pool.is_some() { "connected" } else { "not connected" }, if sn.pool.is_some() { GREEN } else { MUTED }),
            l("miner", sn.miner_pid.map(|p| format!("running, pid {p}")).unwrap_or("not started here".into()), if sn.miner_pid.is_some() { GREEN } else { MUTED }),
            l("peers", if sn.chain_ok() { u(&sn.info, "peers").to_string() } else { "unavailable".into() }, if sn.chain_ok() { TEXT } else { MUTED }),
        ],
    }
}

/// A table column: header, width, right-aligned.
#[derive(Debug, Clone, PartialEq)]
pub struct Col {
    pub name: &'static str,
    pub width: u16,
    pub right: bool,
}

fn col(name: &'static str, width: u16, right: bool) -> Col {
    Col { name, width, right }
}

/// A tab's table: display cells (text, color), the stable key of each row (job id,
/// height, tx hash, nonce, field), the JSON behind each row for the detail card.
#[derive(Debug, Clone, Default)]
pub struct Table {
    pub item: &'static str,
    pub cols: Vec<Col>,
    pub rows: Vec<Vec<(String, u32)>>,
    pub keys: Vec<String>,
    pub raw: Vec<Value>,
    pub footer: String,
    pub empty: String,
}

fn tone_c(st: &str) -> u32 {
    draw::tone(status_tone(st))
}

fn is_zero_hex(h: &str) -> bool {
    !h.is_empty() && h.chars().all(|c| c == '0')
}

pub fn table(sn: &Snapshot, tab: Tab) -> Table {
    let me = sn.me.clone().unwrap_or_default();
    match tab {
        Tab::Jobs => {
            let raw: Vec<Value> = sn.jobs.iter().rev().cloned().collect();
            Table {
                item: "Job",
                cols: vec![col("job", 11, false), col("status", 9, false), col("pay", 10, true), col("fee", 9, true), col("m x n", 10, false), col("blocks", 6, true), col("until", 6, true)],
                rows: raw
                    .iter()
                    .map(|j| {
                        let st = s(j, "status");
                        vec![
                            (short(s(j, "job_id")), TEXT),
                            (st.to_string(), tone_c(st)),
                            (fmt::compact(u(j, "worker_pay") as u128, fmt::CHAIN_DP), TEXT),
                            (fmt::compact(u(j, "protocol_fee") as u128, fmt::CHAIN_DP), TEXT),
                            (format!("{}x{}", u(j, "m"), u(j, "n")), MUTED),
                            (format!("{}/{}", u(j, "blocks_mined"), u(j, "max_blocks")), TEXT),
                            (u(j, "deadline_height").to_string(), MUTED),
                        ]
                    })
                    .collect(),
                keys: raw.iter().map(|j| s(j, "job_id").to_string()).collect(),
                raw,
                footer: format!("{} of {} jobs", sn.jobs.len(), sn.jobs_total),
                empty: if sn.chain_ok() { "no jobs on chain yet (cadre job post)".into() } else { "chain unavailable".into() },
            }
        }
        Tab::Mining => Table {
            item: "Block",
            cols: vec![col("height", 7, true), col("miner", 11, false), col("work", 13, false), col("reward", 12, true), col("txs", 4, true), col("status", 8, false)],
            rows: sn
                .blocks
                .iter()
                .map(|b| {
                    let m = s(&b["header_fields"], "miner");
                    let job = s(&b["header_fields"], "job_id");
                    let st = if b["final"] == true { "final" } else { "pending" };
                    vec![
                        (u(&b["header_fields"], "height").to_string(), TEXT),
                        (if !me.is_empty() && m == me { "you".into() } else { fmt::short_addr(m) }, if m == me { BLUE } else { TEXT }),
                        (if is_zero_hex(job) { "fallback".into() } else { format!("job {}", fmt::short_n(job, 3)) }, if is_zero_hex(job) { MUTED } else { TEXT }),
                        (fmt::compact(u(b, "reward") as u128, fmt::CHAIN_DP), TEXT),
                        (b["txs"].as_array().map(|a| a.len()).unwrap_or(0).to_string(), MUTED),
                        (st.into(), tone_c(st)),
                    ]
                })
                .collect(),
            keys: sn.blocks.iter().map(|b| u(&b["header_fields"], "height").to_string()).collect(),
            raw: sn.blocks.clone(),
            footer: {
                let (earned, mined) = sn.window_rewards();
                if sn.me.is_some() {
                    format!("yours {mined} of {} blocks, {} {TICKER} ({} pending)", sn.blocks.len(), tok(earned), tok(sn.pending_rewards()))
                } else {
                    format!("newest {} blocks", sn.blocks.len())
                }
            },
            empty: if sn.chain_ok() { "no blocks read yet".into() } else { "chain unavailable".into() },
        },
        Tab::Activity => Table {
            item: "Tx",
            cols: vec![col("height", 7, true), col("kind", 15, false), col("role", 9, false), col("fee", 10, true), col("tx", 11, false), col("status", 8, false)],
            rows: sn
                .txs
                .iter()
                .map(|t| {
                    let st = if t["final"] == true { "final" } else { "pending" };
                    vec![
                        (u(t, "height").to_string(), TEXT),
                        (s(t, "kind").into(), TEXT),
                        (s(t, "role").into(), MUTED),
                        (tok(u(t, "fee")), TEXT),
                        (short(s(t, "tx_hash")), MUTED),
                        (st.into(), tone_c(st)),
                    ]
                })
                .collect(),
            keys: sn.txs.iter().map(|t| s(t, "tx_hash").to_string()).collect(),
            raw: sn.txs.clone(),
            footer: if sn.me.is_some() { format!("{} txs touch this account", sn.txs.len()) } else { String::new() },
            empty: if sn.me.is_none() { NO_WALLET.into() } else { "no transactions for this account yet".into() },
        },
        Tab::Bridge => {
            let raw: Vec<Value> = sn.withdrawals.iter().rev().cloned().collect();
            let rel = |n: u64| sn.sol.released.iter().find(|(k, _)| *k == n).map(|(_, r)| *r);
            Table {
                item: "Withdrawal",
                cols: vec![col("nonce", 6, true), col("amount", 12, true), col("from", 11, false), col("to (Solana)", 11, false), col("height", 7, true), col("status", 9, false)],
                rows: raw
                    .iter()
                    .map(|w| {
                        let n = u(w, "nonce");
                        let st = match (rel(n), w["final"] == true) {
                            (Some(true), _) => "released",
                            (_, false) => "locked",
                            _ => "final",
                        };
                        vec![
                            (n.to_string(), TEXT),
                            (fmt::compact(u(w, "amount_spl") as u128, fmt::SPL_DP), TEXT),
                            (fmt::short_addr(s(w, "from")), MUTED),
                            (short(s(w, "solana_recipient")), MUTED),
                            (u(w, "height").to_string(), TEXT),
                            (st.into(), tone_c(st)),
                        ]
                    })
                    .collect(),
                keys: raw.iter().map(|w| u(w, "nonce").to_string()).collect(),
                raw,
                footer: format!("{} withdrawals, {} deposits unlocked", sn.withdrawal_count, sn.deposit_count),
                empty: if sn.chain_ok() { "no withdrawals yet (cadre bridge out AMOUNT)".into() } else { "chain unavailable".into() },
            }
        }
        Tab::Agent => {
            let mut rows = vec![];
            let mut raw = vec![];
            let mut keys = vec![];
            let mut add = |k: &str, v: String, c: u32, r: Value| {
                rows.push(vec![(k.to_string(), MUTED), (v, c)]);
                keys.push(k.to_string());
                raw.push(r);
            };
            if sn.me.is_none() {
                add("account", NO_WALLET.into(), MUTED, Value::Null);
            } else if sn.agent.is_object() {
                add("name", s(&sn.agent, "name").into(), TEXT, sn.agent.clone());
                add("uri", s(&sn.agent, "uri").into(), TEXT, sn.agent.clone());
                add("registered", format!("height {}", u(&sn.agent, "registered_height")), TEXT, sn.agent.clone());
            } else {
                add("registration", "not registered (cadre register NAME)".into(), MUTED, Value::Null);
            }
            if sn.route.is_object() {
                let r = &sn.route;
                let active = r["active"] == true;
                add("lien", if active { "active".into() } else { "ended".into() }, if active { GREEN } else { MUTED }, r.clone());
                add("route", format!("{}% to {}", u(r, "route_bps") as f64 / 100.0, short(s(r, "solana_recipient"))), TEXT, r.clone());
                add("until", format!("height {}", u(r, "until_height")), TEXT, r.clone());
                add("routed", format!("{} {TICKER}", tok(u(r, "routed"))), TEXT, r.clone());
                add("withdrawn", format!("{} {TICKER}", tok(u(r, "withdrawn"))), TEXT, r.clone());
                add("cap left", format!("{} {TICKER}", tok(u(r, "remaining_cap"))), TEXT, r.clone());
            } else if sn.me.is_some() {
                add("lien", "none".into(), MUTED, Value::Null);
            }
            add("agent credit", sn.credit.clone(), if sn.credit == "reachable" { GREEN } else { MUTED }, Value::Null);
            Table { item: "Agent", cols: vec![col("field", 16, false), col("value", 48, false)], rows, keys, raw, footer: "payout lien = SetRoute (SPEC 4.4)".into(), empty: String::new() }
        }
        Tab::Send => Table { item: "Send", ..Default::default() },
    }
}

/// Key: value pairs of a JSON row for the detail card.
pub fn detail_lines(v: &Value) -> Vec<(String, String)> {
    let plain = |v: &Value| match v {
        Value::String(s) => s.clone(),
        Value::Null => "-".into(),
        Value::Array(a) => format!("[{} items]", a.len()),
        o => o.to_string(),
    };
    match v {
        Value::Object(o) => o
            .iter()
            .filter(|(k, _)| !matches!(k.as_str(), "tx" | "header" | "path" | "config" | "job_header" | "coinbase" | "txs"))
            .flat_map(|(k, v)| match v {
                Value::Object(inner) => inner.iter().map(|(k2, v2)| (format!("{k}.{k2}"), plain(v2))).collect::<Vec<_>>(),
                _ => vec![(k.clone(), plain(v))],
            })
            .collect(),
        Value::Null => vec![("".into(), "nothing more to show".into())],
        o => vec![("".into(), o.to_string())],
    }
}

// ---------------------------------------------------------------------------------------
// state

#[derive(Debug, Default, Clone)]
pub struct SendForm {
    pub to: Vec<char>,
    pub amount: Vec<char>,
    /// 0 = to, 1 = amount.
    pub field: usize,
    /// Enter once more signs and sends.
    pub confirm: bool,
    pub busy: bool,
    pub result: Option<Result<String, String>>,
}

impl SendForm {
    pub fn cur(&mut self) -> &mut Vec<char> {
        if self.field == 0 {
            &mut self.to
        } else {
            &mut self.amount
        }
    }
}

/// Everything the Dashboard remembers between visits.
#[derive(Debug)]
pub struct DashState {
    pub tab: Tab,
    /// The selected row of each tab, by its key (stays on the same job when new ones arrive).
    pub sel: HashMap<Tab, String>,
    /// First visible row of each tab.
    pub scroll: HashMap<Tab, usize>,
    /// The open detail card: (title, key: value lines, first line shown).
    pub detail: Option<(String, Vec<(String, String)>, usize)>,
    pub send: SendForm,
    /// Rows the table showed last time (for paging and keeping the selection in view).
    pub visible: Cell<usize>,
}

impl Default for DashState {
    fn default() -> Self {
        DashState { tab: Tab::Jobs, sel: HashMap::new(), scroll: HashMap::new(), detail: None, send: SendForm::default(), visible: Cell::new(10) }
    }
}

impl DashState {
    /// Index of the selected row in `t` (the first row when the key is gone or unset).
    pub fn sel_index(&self, t: &Table) -> usize {
        self.sel.get(&self.tab).and_then(|k| t.keys.iter().position(|x| x == k)).unwrap_or(0)
    }

    pub fn selected_key(&self) -> Option<&String> {
        self.sel.get(&self.tab)
    }

    /// Move the selection by `d` rows (clamped) and keep it in view.
    pub fn move_sel(&mut self, t: &Table, d: isize) {
        if t.keys.is_empty() {
            return;
        }
        let i = self.sel_index(t) as isize;
        let j = (i + d).clamp(0, t.keys.len() as isize - 1) as usize;
        self.sel.insert(self.tab, t.keys[j].clone());
        self.keep_in_view(j);
    }

    pub fn keep_in_view(&mut self, j: usize) {
        let vis = self.visible.get().max(1);
        let sc = self.scroll.entry(self.tab).or_insert(0);
        if j < *sc {
            *sc = j;
        } else if j >= *sc + vis {
            *sc = j + 1 - vis;
        }
    }

    pub fn open_detail(&mut self, t: &Table) {
        let i = self.sel_index(t);
        if let Some(raw) = t.raw.get(i) {
            let label = t.rows[i].iter().map(|c| c.0.clone()).next().unwrap_or_default();
            self.detail = Some((format!("{} {label}", t.item), detail_lines(raw), 0));
        }
    }
}

// ---------------------------------------------------------------------------------------
// drawing

/// What the dashboard draws besides its own state.
pub struct Ctx<'a> {
    pub snap: Option<&'a Snapshot>,
    pub has_key: bool,
    pub min_fee: Option<u64>,
    pub ms: u128,
}

fn draw_card(buf: &mut Buffer, pen: &Pen, r: Rect, c: &Card) {
    let inner = card(buf, pen, r);
    if inner.height == 0 {
        return;
    }
    put(buf, inner.x, inner.y, &clip(&c.title, inner.width as usize), pen.card(MUTED), inner.right());
    let mut y = inner.y + 1;
    if y < inner.bottom() {
        put(buf, inner.x, y, &clip(&c.head.0, inner.width as usize), pen.card(c.head.1).add_modifier(if c.head.1 == TEXT { Modifier::BOLD } else { Modifier::empty() }), inner.right());
        y += 1;
    }
    for (label, value, color) in &c.lines {
        if y >= inner.bottom() {
            break;
        }
        if label.is_empty() {
            put(buf, inner.x, y, &clip(value, inner.width as usize), pen.card(*color), inner.right());
        } else {
            kv(buf, pen, inner, y, label, value, *color);
        }
        y += 1;
    }
}

fn waiting_card(title: &str, ms: u128) -> Card {
    Card { title: title.into(), head: (format!("{} reading", draw::spinner(ms)), MUTED), lines: vec![] }
}

/// Rows of the top cards: title, headline, four lines, the padding and the bottom edge.
pub fn card_height(body_h: u16) -> u16 {
    9u16.min(body_h / 3).max(4)
}

/// The body of the Dashboard view in `area`.
pub fn draw(buf: &mut Buffer, pen: &Pen, area: Rect, st: &DashState, cx: &Ctx) {
    if area.width < 20 || area.height < 6 {
        return;
    }
    let sn = cx.snap;
    let cards: Vec<Card> = match sn {
        Some(sn) => vec![wallet(sn), market(sn), protocol(sn), mining(sn)],
        None => ["Wallet", "Market", "Protocol", "Mining"].iter().map(|t| waiting_card(t, cx.ms)).collect(),
    };
    // top row of cards
    let n = ((area.width + 2) / 28).clamp(2, 4) as usize;
    let gap = 2u16;
    let cw = (area.width - gap * (n as u16 - 1)) / n as u16;
    let ch = card_height(area.height);
    for (i, c) in cards.iter().take(n).enumerate() {
        let x = area.x + i as u16 * (cw + gap);
        let w = if i == n - 1 { area.right() - x } else { cw };
        draw_card(buf, pen, Rect::new(x, area.y, w, ch), c);
    }
    let rest = Rect::new(area.x, area.y + ch + 1, area.width, area.height.saturating_sub(ch + 1));
    // right column: bridge and agent cards
    let (left, right) = if area.width >= 100 {
        let rw = (area.width * 30 / 100).clamp(34, 52);
        (Rect::new(rest.x, rest.y, rest.width - rw - gap, rest.height), Some(Rect::new(rest.right() - rw, rest.y, rw, rest.height)))
    } else {
        (rest, None)
    };
    if let Some(r) = right {
        let (b, a) = match sn {
            Some(sn) => (bridge(sn), agent(sn)),
            None => (waiting_card("Bridge", cx.ms), waiting_card("Agent", cx.ms)),
        };
        let bh = (b.lines.len() as u16 + 5).min(r.height);
        draw_card(buf, pen, Rect::new(r.x, r.y, r.width, bh), &b);
        let ah = (a.lines.len() as u16 + 5).min(r.height.saturating_sub(bh + 1));
        if ah >= 4 {
            draw_card(buf, pen, Rect::new(r.x, r.y + bh + 1, r.width, ah), &a);
        }
    }
    draw_tabs(buf, pen, left, st, cx);
}

fn draw_tabs(buf: &mut Buffer, pen: &Pen, r: Rect, st: &DashState, cx: &Ctx) {
    if r.height < 3 {
        return;
    }
    // tab bar: the active tab in blue (the mode-label color), the rest muted
    let mut x = r.x;
    for t in Tab::ALL {
        let on = t == st.tab;
        let stl = if on { pen.fg(BLUE).add_modifier(Modifier::BOLD) } else { pen.fg(MUTED) };
        x = put(buf, x, r.y, t.name(), stl, r.right());
        x += 3;
    }
    let body = Rect::new(r.x, r.y + 2, r.width, r.height.saturating_sub(2));
    if st.tab == Tab::Send {
        draw_send(buf, pen, body, st, cx);
        return;
    }
    let Some(sn) = cx.snap else {
        put(buf, body.x + 2, body.y, &format!("{} reading the chain", draw::spinner(cx.ms)), pen.fg(MUTED), body.right());
        return;
    };
    let t = table(sn, st.tab);
    if !t.footer.is_empty() {
        put_right(buf, r.right(), r.y, &clip(&t.footer, r.right().saturating_sub(x + 2) as usize), pen.fg(DIM), x + 2);
    }
    draw_table(buf, pen, body, st, &t);
    // the first read error, if any, under the table
    if let Some(e) = sn.errors.first() {
        let y = r.bottom().saturating_sub(1);
        let x = put(buf, r.x + 2, y, "! ", pen.fg(RED), r.right());
        put(buf, x, y, &clip(e, r.right().saturating_sub(x) as usize), pen.fg(MUTED), r.right());
    }
}

/// Column widths fitted to `w` cells (the last column takes what is left).
fn fit(cols: &[Col], w: u16) -> Vec<u16> {
    let gap = 2u16;
    let mut out: Vec<u16> = cols.iter().map(|c| c.width).collect();
    let need: u16 = out.iter().sum::<u16>() + gap * (out.len() as u16).saturating_sub(1);
    if need > w {
        // shrink the widest columns first
        let mut over = need - w;
        while over > 0 {
            let (i, m) = out.iter().enumerate().max_by_key(|(_, v)| **v).map(|(i, v)| (i, *v)).unwrap();
            if m <= 4 {
                break;
            }
            out[i] -= 1;
            over -= 1;
        }
    }
    out
}

pub fn draw_table(buf: &mut Buffer, pen: &Pen, r: Rect, st: &DashState, t: &Table) {
    let x0 = r.x + 2;
    let inner_w = r.width.saturating_sub(4);
    let widths = fit(&t.cols, inner_w);
    // header
    let mut x = x0;
    for (c, w) in t.cols.iter().zip(&widths) {
        let h = clip(c.name, *w as usize);
        if c.right {
            put_right(buf, x + w, r.y, &h, pen.fg(MUTED), x);
        } else {
            put(buf, x, r.y, &h, pen.fg(MUTED), x + w);
        }
        x += w + 2;
    }
    let vis = r.height.saturating_sub(2) as usize;
    st.visible.set(vis.max(1));
    if t.rows.is_empty() {
        put(buf, x0, r.y + 2, &t.empty, pen.fg(MUTED), r.right());
        return;
    }
    let sel = st.sel_index(t);
    let mut first = st.scroll.get(&st.tab).copied().unwrap_or(0).min(t.rows.len().saturating_sub(1));
    // the selection always in view, even when rows arrived since the last key
    if sel < first {
        first = sel;
    } else if sel >= first + vis {
        first = sel + 1 - vis;
    }
    for (i, row) in t.rows.iter().enumerate().skip(first).take(vis) {
        let y = r.y + 1 + (i - first) as u16;
        let on = i == sel;
        if on {
            selected_row(buf, pen, r.x, y, r.right());
        }
        let mut x = x0;
        for ((cell, c), (w, col)) in row.iter().zip(widths.iter().zip(&t.cols)) {
            let text = clip(cell, *w as usize);
            let mut stl = if on { pen.on(*c, PANEL) } else { pen.fg(*c) };
            if on && *c == TEXT {
                stl = stl.add_modifier(Modifier::BOLD);
            }
            if col.right {
                put_right(buf, x + w, y, &text, stl, x);
            } else {
                put(buf, x, y, &text, stl, x + w);
            }
            x += w + 2;
        }
    }
    // a quiet position marker when the table scrolls
    if t.rows.len() > vis {
        let s = format!("{}-{} of {}", first + 1, (first + vis).min(t.rows.len()), t.rows.len());
        put_right(buf, r.right(), r.bottom().saturating_sub(1).max(r.y), &s, pen.fg(DIM), r.x);
    }
}

fn field(buf: &mut Buffer, pen: &Pen, x: u16, y: u16, w: u16, label: &str, v: &[char], focus: bool, suffix: &str) -> Option<ratatui::layout::Position> {
    put(buf, x, y, label, pen.fg(MUTED), x + 10);
    let bx = x + 10;
    draw::fill(buf, Rect::new(bx + 1, y, w, 1), pen.bg(PANEL));
    put(buf, bx, y, "┃", pen.fg(if focus { BLUE } else { DIM }), bx + 1);
    let s: String = v.iter().collect();
    let end = put(buf, bx + 2, y, &s, pen.card(TEXT), bx + w);
    put(buf, bx + w + 2, y, suffix, pen.fg(MUTED), bx + w + 2 + str_w(suffix));
    focus.then(|| ratatui::layout::Position::new(end, y))
}

fn draw_send(buf: &mut Buffer, pen: &Pen, r: Rect, st: &DashState, cx: &Ctx) {
    let x = r.x + 2;
    if !cx.has_key {
        put(buf, x, r.y, NO_WALLET, pen.fg(MUTED), r.right());
        put(buf, x, r.y + 1, "sending signs a Transfer on the agent chain with that key", pen.fg(DIM), r.right());
        return;
    }
    let f = &st.send;
    let w = r.width.saturating_sub(24).min(56);
    field(buf, pen, x, r.y, w, "to", &f.to, f.field == 0 && !f.confirm, "address or @handle");
    field(buf, pen, x, r.y + 2, w.min(24), "amount", &f.amount, f.field == 1 && !f.confirm, TICKER);
    let fee = match cx.min_fee {
        Some(m) => format!("fee {} {TICKER} (the chain's min_fee)", tok(m)),
        None => "fee: the chain's min_fee".into(),
    };
    put(buf, x + 10, r.y + 4, &fee, pen.fg(DIM), r.right());
    let y = r.y + 6;
    if f.busy {
        put(buf, x, y, &format!("{} signing and sending", draw::spinner(cx.ms)), pen.fg(BLUE), r.right());
    } else if f.confirm {
        let to: String = f.to.iter().collect();
        let amt: String = f.amount.iter().collect();
        let s = format!("enter signs and sends {amt} {TICKER} to {}; esc edits", short(&to));
        put(buf, x, y, &s, pen.fg(YELLOW), r.right());
    } else if let Some(res) = &f.result {
        let (s, c) = match res {
            Ok(s) => (s.as_str(), GREEN),
            Err(e) => (e.as_str(), RED),
        };
        for (i, line) in draw::wrap(s, r.width.saturating_sub(4) as usize).iter().take(3).enumerate() {
            put(buf, x, y + i as u16, line, pen.fg(c), r.right());
        }
    }
}

/// The detail card over the dashboard.
pub fn draw_detail(buf: &mut Buffer, pen: &Pen, screen: Rect, d: &(String, Vec<(String, String)>, usize)) {
    let w = (screen.width * 3 / 4).clamp(40.min(screen.width), 100.min(screen.width));
    let h = (d.1.len() as u16 + 6).min(screen.height.saturating_sub(6)).max(6.min(screen.height));
    let r = Rect::new(screen.x + (screen.width - w) / 2, screen.y + (screen.height.saturating_sub(h)) / 3, w, h);
    let inner = card(buf, pen, r);
    if inner.height == 0 {
        return;
    }
    draw::title(buf, pen, inner, &d.0);
    put_right(buf, inner.right(), inner.y, "esc", pen.card(MUTED), inner.x);
    let lw = d.1.iter().map(|(k, _)| str_w(k)).max().unwrap_or(0).min(inner.width / 2);
    for (i, (k, v)) in d.1.iter().skip(d.2).enumerate() {
        let y = inner.y + 2 + i as u16;
        if y >= inner.bottom() {
            break;
        }
        put(buf, inner.x, y, &clip(k, lw as usize), pen.card(MUTED), inner.x + lw);
        put(buf, inner.x + lw + 2, y, &clip(v, inner.width.saturating_sub(lw + 2) as usize), pen.card(TEXT), inner.right());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::snapshot::sample;

    #[test]
    fn cards_map_the_snapshot() {
        let sn = sample::snapshot();
        let w = wallet(&sn);
        assert_eq!(w.head, ("40.74 CADRE".to_string(), TEXT));
        assert_eq!(w.lines[0], l("pending", "2 CADRE", TEXT));
        assert_eq!(w.lines[1], l("on Solana", "8.69M CADRE", TEXT));
        assert_eq!(w.lines[2], l("SOL", "2.5", TEXT));
        let m = market(&sn);
        assert_eq!(m.head.0, "job from 0.01 SOL");
        assert_eq!(m.lines[0], l("orders", "2", TEXT));
        assert_eq!(m.lines[1], l("CADRE per SOL", "30M", TEXT));
        assert_eq!(m.lines[2], l("price", "no price source", DIM));
        let p = protocol(&sn);
        assert_eq!(p.head.0, "block 260");
        assert_eq!(p.lines[0].1, "254  K 6");
        assert_eq!(p.lines[1].1, "0x1f00ffff");
        assert_eq!(p.lines[2], l("max subsidy", "90K CADRE", TEXT));
        assert_eq!(p.lines[3], l("peers", "2", TEXT));
        let mi = mining(&sn);
        assert_eq!(mi.head.0, "3 of 8 recent blocks");
        assert_eq!(mi.lines[0].1, "3 CADRE");
        let b = bridge(&sn);
        assert_eq!(b.head.0, "981.16M CADRE");
        assert_eq!(b.lines[0].1, "1.88%");
        assert_eq!(b.lines[3].1, "5 (+0 in flight)");
        assert_eq!(b.lines[4].1, "2 (1 unreleased)");
        assert_eq!(b.lines[5], l("invariant", "OK (in flight)", GREEN));
        let a = agent(&sn);
        assert_eq!(a.head.0, "demo-agent");
        assert_eq!(a.lines[0].1, "50% to 7xKX...gAsU");
        assert_eq!(a.lines[4].1, "2");
    }

    #[test]
    fn empty_states_say_what_is_missing() {
        let mut sn = Snapshot::default();
        sn.errors.push("agent chain: connection refused".into());
        assert_eq!(wallet(&sn).head.0, NO_WALLET);
        assert_eq!(protocol(&sn).head, ("chain offline".to_string(), RED));
        assert_eq!(protocol(&sn).lines[0].1, "agent chain: connection refused");
        assert_eq!(market(&sn).head.0, "storefront unavailable");
        assert_eq!(bridge(&sn).head.0, "unavailable");
        assert_eq!(bridge(&sn).lines[5].1, "unavailable");
        assert_eq!(table(&sn, Tab::Jobs).empty, "chain unavailable");
        assert_eq!(table(&sn, Tab::Activity).empty, NO_WALLET);
        // no figure anywhere is a made-up zero
        for c in [wallet(&sn), market(&sn), protocol(&sn), bridge(&sn), agent(&sn)] {
            assert!(!c.head.0.starts_with('0'), "{c:?}");
        }
    }

    #[test]
    fn tables_and_selection_by_key() {
        let mut sn = sample::snapshot();
        let t = table(&sn, Tab::Jobs);
        assert_eq!(t.keys, vec!["bb".repeat(32), "aa".repeat(32)]);
        assert_eq!(t.rows[0][1], ("open".to_string(), YELLOW));
        assert_eq!(t.rows[1][1], ("completed".to_string(), GREEN));
        assert_eq!(t.rows[0][4].0, "128x64");
        let mut st = DashState::default();
        st.move_sel(&t, 1);
        assert_eq!(st.selected_key(), Some(&"aa".repeat(32)));
        // a new job arrives first in the list: the selection stays on the same job
        sn.jobs.push(serde_json::json!({"job_id": "cc".repeat(32), "status": "open"}));
        let t2 = table(&sn, Tab::Jobs);
        assert_eq!(st.sel_index(&t2), 2);
        st.open_detail(&t2);
        let d = st.detail.clone().unwrap();
        assert!(d.0.starts_with("Job aaaa"));
        assert!(d.1.iter().any(|(k, v)| k == "status" && v == "completed"));
        let m = table(&sn, Tab::Mining);
        assert_eq!(m.rows[0][1].0, "you");
        assert_eq!(m.rows[2][2].0, "job 000...bcd");
        assert!(m.footer.contains("yours 3 of 8 blocks"));
        let b = table(&sn, Tab::Bridge);
        assert_eq!(b.rows[0][5].0, "locked");
        assert_eq!(b.rows[1][5].0, "released");
        assert_eq!(Tab::parse("BRIDGE").unwrap(), Tab::Bridge);
        assert!(Tab::parse("x").is_err());
    }

    #[test]
    fn fits_columns() {
        let cols = vec![col("a", 10, false), col("b", 10, false)];
        assert_eq!(fit(&cols, 30), vec![10, 10]);
        assert_eq!(fit(&cols, 16), vec![7, 7]);
    }
}
