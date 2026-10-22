//! Agent trading (docs/TRADING.md): `cadre trades` (fills on both chains, newest first) and the
//! trading block of `cadre agent` (positions, realized PnL, the position at the pool mid,
//! recent fills). Read-only, from cadre-api's /v1/trading routes, which compute everything from
//! chain data; house traders are labelled.

use anyhow::{anyhow, Result};
use serde_json::{json, Value};

use crate::commands::Out;
use crate::ctx::Ctx;
use crate::fmt::{amount, short_n, sol};
use crate::probe::get;
use crate::snapshot::{s, u};
use crate::term::{Block, Panel, Table, Tone, View};

fn n(v: &Value) -> i128 {
    v.as_str().and_then(|x| x.parse().ok()).or_else(|| v.as_i64().map(|x| x as i128)).unwrap_or(0)
}
fn signed(x: i128, f: impl Fn(u128) -> String) -> String {
    if x < 0 { format!("-{}", f((-x) as u128)) } else { f(x as u128) }
}
/// CADRE on Solana (pump.fun mint, 6 decimals) and on the Cadre chain (9 decimals).
fn spl6(x: u128) -> String {
    amount(x, 6, 2)
}
fn chain9(x: u128) -> String {
    amount(x, 9, 4)
}

fn trade_row(t: &Value) -> Vec<(String, Tone)> {
    let venue = s(t, "venue");
    let sol_side = venue == "pumpswap";
    let (inp, out) = (n(&t["amount_in"]) as u128, n(&t["amount_out"]) as u128);
    let what = match (s(t, "side"), sol_side) {
        ("buy", true) => format!("{} SOL -> {} CADRE", sol(inp), spl6(out)),
        ("sell", true) => format!("{} CADRE -> {} SOL", spl6(inp), sol(out)),
        ("buy", false) => format!("{} CADRE -> {} token", chain9(inp), chain9(out)),
        _ => format!("{} token -> {} CADRE", chain9(inp), chain9(out)),
    };
    let flags = t["flags"].as_array().map(|f| f.len()).unwrap_or(0);
    vec![
        (venue.to_string(), Tone::Plain),
        (format!("{}{}", short_n(s(t, "agent"), 10), if t["house"] == true { " (house)" } else { "" }), if t["house"] == true { Tone::Dim } else { Tone::Plain }),
        (s(t, "via").to_string(), Tone::Dim),
        (s(t, "side").to_string(), if s(t, "side") == "buy" { Tone::Good } else { Tone::Warn }),
        (what, Tone::Plain),
        (short_n(s(t, "ref"), 22), Tone::Dim),
        (if flags > 0 { format!("{flags} flag(s)") } else { String::new() }, if flags > 0 { Tone::Warn } else { Tone::Dim }),
    ]
}

fn trades_table(chip: &str, list: &[Value]) -> Table {
    Table {
        chip: Some(chip.into()),
        head: ["VENUE", "AGENT", "VIA", "SIDE", "FILL", "TX", "FLAGS"].map(String::from).to_vec(),
        right: vec![false; 7],
        rows: list.iter().map(trade_row).collect(),
        empty: "no trades on chain".into(),
    }
}

/// `cadre trades [--agent A] [--venue pumpswap|cadre-amm] [--organic] [--limit N]`.
pub fn trades(ctx: &Ctx, agent: Option<&str>, venue: Option<&str>, organic: bool, limit: usize) -> Result<Out> {
    let api = ctx.eps.get("api").ok_or_else(|| anyhow!("needs cadre-api (set CADRE_API)"))?.to_string();
    let mut q = vec![format!("limit={}", limit.clamp(1, 200))];
    if let Some(a) = agent {
        q.push(format!("agent={a}"));
    }
    if let Some(v) = venue {
        q.push(format!("venue={v}"));
    }
    if organic {
        q.push("organic=1".into());
    }
    let h = get(&format!("{api}/trading/trades?{}", q.join("&")));
    if !h.ok() {
        return Err(anyhow!("cadre-api /trading/trades: HTTP {} {}", h.status, short_n(&h.json.to_string(), 160)));
    }
    let list = h.json["trades"].as_array().cloned().unwrap_or_default();
    let summary = get(&format!("{api}/trading"));
    let mut view = View::panel(summary_panel(&summary.json)).with(Block::Table(trades_table("Trades, newest first", &list)));
    view = view.hint("every fill is a chain event: sol:<signature> on Solana, cadre:<tx> on Cadre; house traders are excluded from organic volume");
    Ok(Out::new(json!({"trades": list, "total": h.json["total"], "summary": summary.json}), format!("{} trades", list.len())).view(view))
}

fn summary_panel(j: &Value) -> Panel {
    let mut p = Panel::new("Trading venues");
    let sv = &j["venues"]["solana"];
    if sv.is_object() {
        let r = &sv["reserves"];
        p = p.kv("PumpSwap CADRE", format!("pool {} ({} CADRE / {} SOL)", short_n(s(sv, "pool"), 12), spl6(n(&r["cadre_spl"]) as u128), sol(n(&r["sol_lamports"]) as u128)));
        p = p.kv("Mandate program", s(sv, "program"));
    }
    let cv = &j["venues"]["cadre"];
    if cv.is_object() {
        let r = &cv["reserves"];
        p = p.kv("Cadre AMM", format!("pool {} ({} CADRE / {} token, fee {} bps)", short_n(s(cv, "amm_pool"), 12), chain9(n(&r["reserve_cadre"]) as u128), chain9(n(&r["reserve_token"]) as u128), u(r, "fee_bps")));
    }
    let v = &j["volume"];
    if v.is_object() {
        p = p.kv("Volume (Solana)", format!("organic {} SOL, house {} SOL", sol(n(&v["solana_lamports"]["organic"]) as u128), sol(n(&v["solana_lamports"]["house"]) as u128)));
        p = p.kv("Volume (Cadre)", format!("organic {} CADRE, house {} CADRE", chain9(n(&v["cadre_base_units"]["organic"]) as u128), chain9(n(&v["cadre_base_units"]["house"]) as u128)));
    }
    p
}

/// The trading part of `cadre agent`: None when the account never traded.
pub fn agent_blocks(api: &str, account: &str) -> Option<(Vec<Block>, Value)> {
    let h = get(&format!("{api}/trading/agents/{account}"));
    if !h.ok() {
        return None;
    }
    let j = h.json;
    let mut p = Panel::new("Trading").kv("House trader", if j["house"] == true { "yes (labelled, excluded from organic volume)" } else { "no" });
    for m in j["positions"]["solana_mandates"].as_array().into_iter().flatten() {
        p = p.section(&format!("Solana mandate {}", short_n(s(m, "mandate"), 12)));
        p = p.kv("User", s(m, "user")).state("Agent", if m["revoked"] == true { "revoked" } else { "active" });
        p = p.kv("SOL in the account", sol(n(&m["sol_equity_lamports"]) as u128)).kv("CADRE held", spl6(n(&m["cadre_position_spl"]) as u128));
        p = p.kt("Realized PnL", format!("{} SOL", signed(n(&m["realized_pnl_lamports"]), sol)), if n(&m["realized_pnl_lamports"]) < 0 { Tone::Warn } else { Tone::Good });
        if !m["marked_at_mid_lamports"].is_null() {
            p = p.kt("CADRE at pool mid", format!("{} SOL (a mark, not profit)", sol(n(&m["marked_at_mid_lamports"]) as u128)), Tone::Dim);
        }
        p = p.kv("Fees paid", format!("{} SOL to the agent, {} SOL to the buyback", sol(n(&m["fees_agent_lamports"]) as u128), sol(n(&m["fees_protocol_lamports"]) as u128)));
        p = p.kv("Trades", format!("{} ({} SOL volume)", u(m, "trades"), sol(n(&m["volume_lamports"]) as u128)));
    }
    for w in j["positions"]["cadre_wallet"].as_array().into_iter().flatten() {
        p = p.section("Cadre AMM (own wallet)");
        p = p.kv("Token held", chain9(n(&w["position_token"]) as u128)).kt("Realized PnL", format!("{} CADRE (average cost)", signed(n(&w["realized_pnl_cadre"]), chain9)), Tone::Plain);
    }
    for m in j["positions"]["cadre_mandates"].as_array().into_iter().flatten() {
        p = p.section(&format!("Cadre mandate #{}", u(m, "id")));
        p = p.kv("User", s(m, "user")).kv("CADRE in the account", chain9(n(&m["equity"]) as u128)).kv("Token held", chain9(n(&m["position"]) as u128));
        p = p.kt("Realized PnL", format!("{} CADRE", signed(n(&m["realized_pnl"]), chain9)), Tone::Plain);
    }
    let list = j["trades"].as_array().cloned().unwrap_or_default();
    let take: Vec<Value> = list.into_iter().take(10).collect();
    Some((vec![Block::Panel(p), Block::Table(trades_table("Recent fills", &take))], j))
}
