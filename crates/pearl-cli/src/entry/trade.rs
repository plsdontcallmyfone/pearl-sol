//! The Trade view of the Cadre app (docs/TRADING.md, docs/CLI.md "Trade"). Trades are on
//! Solana only (owner 2026-10-21): CADRE and pump coins on PumpSwap. The view shows
//!
//!   - a CADRE on PumpSwap quote for a fixed input in each direction (the fee-free constant
//!     product quote cadre-api computes from the pool's reserves);
//!   - your positions and balances (the key or --address this app runs with);
//!   - the leaderboard, user agents (organic) apart from house traders;
//!   - the latest fills, each with the agent, its kind and what decided it.
//!
//! Every figure comes from cadre-api, which reads it from the chains; a section whose read
//! failed says "unavailable" with the reason. The view reads only when it is open (the
//! poller's Trade source, every 15 s). Buying and selling run as commands (`cadre buy`,
//! `cadre sell`), which always show the plain-words review and ask before signing.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Modifier;
use serde_json::Value;

use super::draw::{self, card, clip, put, put_right, str_w, Pen};
use super::theme::{BLUE, DIM, GREEN, MUTED, RED, TEXT, YELLOW};
use super::views::{blit, canvas, max_scroll};
use crate::fmt::{amount, short_addr, short_n, sol};

/// The fixed inputs the quote card prices (inputs, not measurements).
pub const QUOTE_SOL_IN: u64 = 10_000_000; // 0.01 SOL
pub const QUOTE_TOKENS_IN: u64 = 100_000_000_000; // 100,000 CADRE (6 decimals)

/// What one Trade read delivered (each part may have failed on its own).
#[derive(Debug, Clone)]
pub struct TradeData {
    pub buy: Result<Value, String>,
    pub sell: Result<Value, String>,
    pub board: Result<Value, String>,
    pub trades: Result<Value, String>,
    /// /trading/agents/{me}: Ok(None) when the account never traded (404).
    pub mine: Option<Result<Option<Value>, String>>,
    pub balances: Option<Result<Value, String>>,
}

#[derive(Debug, Default)]
pub struct TradeState {
    pub data: Option<TradeData>,
    pub scroll: u16,
}

/// One read of everything the view shows. `api` is cadre-api's /v1 root.
pub fn fetch(api: Option<&str>, me: Option<&str>) -> TradeData {
    let get = |path: &str| -> Result<Value, String> {
        let a = api.ok_or("no cadre-api on this network")?;
        super::net::get(&format!("{a}{path}")).map_err(|e| format!("{e:#}"))
    };
    std::thread::scope(|sc| {
        let buy = sc.spawn(|| get(&format!("/pump/quote?side=buy&sol_in={QUOTE_SOL_IN}")));
        let sell = sc.spawn(|| get(&format!("/pump/quote?side=sell&tokens_in={QUOTE_TOKENS_IN}")));
        let board = sc.spawn(|| get("/trading/leaderboard"));
        let trades = sc.spawn(|| get("/trading/trades?limit=20"));
        let mine = me.map(|m| {
            let p = format!("/trading/agents/{m}");
            sc.spawn(move || match get(&p) {
                Ok(v) => Ok(Some(v)),
                Err(e) if is_not_found(&e) => Ok(None),
                Err(e) => Err(e),
            })
        });
        let bal = me.map(|m| {
            let p = format!("/accounts/{m}/balances");
            sc.spawn(move || get(&p))
        });
        TradeData {
            buy: buy.join().unwrap(),
            sell: sell.join().unwrap(),
            board: board.join().unwrap(),
            trades: trades.join().unwrap(),
            mine: mine.map(|h| h.join().unwrap()),
            balances: bal.map(|h| h.join().unwrap()),
        }
    })
}

fn is_not_found(e: &str) -> bool {
    e.contains("404") || e.contains("not_found") || e.to_ascii_lowercase().contains("no trading on chain")
}

// ---------------------------------------------------------------------------------------
// formatting (pure, tested)

pub fn n(v: &Value) -> i128 {
    v.as_str().and_then(|x| x.parse().ok()).or_else(|| v.as_i64().map(|x| x as i128)).or_else(|| v.as_u64().map(|x| x as i128)).unwrap_or(0)
}
fn has(v: &Value) -> bool {
    !v.is_null() && (v.is_number() || v.as_str().is_some_and(|s| !s.is_empty()))
}
fn signed(x: i128, f: impl Fn(u128) -> String) -> String {
    if x < 0 {
        format!("-{}", f((-x) as u128))
    } else {
        f(x as u128)
    }
}
/// SPL amounts of pump coins (6 decimals), e.g. CADRE on Solana.
pub fn spl(x: u128) -> String {
    amount(x, 6, 2)
}
fn sol_s(v: &Value) -> String {
    if has(v) {
        format!("{} SOL", signed(n(v), sol))
    } else {
        "unavailable".into()
    }
}
fn s<'a>(v: &'a Value, k: &str) -> &'a str {
    v.get(k).and_then(|x| x.as_str()).unwrap_or("")
}

/// A line on a card: label, value, value color.
pub type Line = (String, String, u32);

/// The quote card: what 0.01 SOL buys and what 100,000 CADRE sells for, from the pool.
pub fn quote_lines(buy: &Result<Value, String>, sell: &Result<Value, String>) -> Vec<Line> {
    let mut l: Vec<Line> = vec![];
    match buy {
        Ok(q) => {
            l.push(("Venue".into(), format!("{} (Solana)", if s(q, "venue").is_empty() { "unknown" } else { s(q, "venue") }), TEXT));
            if let Some(m) = q.get("mint").and_then(|m| m.as_str()) {
                l.push(("Mint".into(), short_n(m, 16), MUTED));
            }
            let r = &q["reserves"];
            if has(&r["base_spl"]) {
                l.push(("Pool reserves".into(), format!("{} CADRE / {} SOL", spl(n(&r["base_spl"]) as u128), sol(n(&r["quote_lamports"]) as u128)), MUTED));
                let (b, qt) = (n(&r["base_spl"]) as f64, n(&r["quote_lamports"]) as f64);
                if b > 0.0 {
                    // lamports per base unit x 1e6 base units per CADRE / 1e9 lamports per SOL
                    l.push(("Mid price".into(), format!("{:.9} SOL per CADRE", qt / b * 1e6 / 1e9), TEXT));
                }
            }
            l.push((format!("Buy with {} SOL", sol(QUOTE_SOL_IN as u128)), format!("{} CADRE before fees", spl(n(&q["tokens_out_spl"]) as u128)), GREEN));
        }
        Err(e) => l.push(("Buy quote".into(), format!("unavailable: {e}"), RED)),
    }
    match sell {
        Ok(q) => l.push((format!("Sell {} CADRE", spl(QUOTE_TOKENS_IN as u128)), format!("{} SOL before fees", sol(n(&q["sol_out_lamports"]) as u128)), YELLOW)),
        Err(e) => l.push(("Sell quote".into(), format!("unavailable: {e}"), RED)),
    }
    l.push(("Fees".into(), "pump's fee (1.25% curve, tiered PumpSwap) and slippage come off".into(), DIM));
    l
}

/// "user agent", "house" or "mandate" for a leaderboard or trade row.
pub fn kind_of(r: &Value) -> &'static str {
    match s(r, "kind") {
        "user_agent" | "user" => "user agent",
        "house" => "house",
        "person" | "wallet" => "person",
        "" if r["house"] == true => "house",
        "" => "mandate",
        _ => "other",
    }
}

/// Leaderboard rows: (who, kind, realized PnL, position, trades) with the PnL color.
pub fn board_rows(board: &Value) -> (Vec<[String; 5]>, Vec<[String; 5]>, Vec<u32>, Vec<u32>) {
    let name = |r: &Value| -> String {
        let h = s(r, "handle");
        let who = if h.is_empty() { short_n(s(r, "agent"), 12) } else { format!("@{h}") };
        if s(r, "name").is_empty() { who } else { format!("{who} {}", s(r, "name")) }
    };
    let row = |r: &Value| -> ([String; 5], u32) {
        let pnl = n(&r["realized_pnl_lamports"]);
        let pos = if has(&r["position_spl"]) { &r["position_spl"] } else { &r["position_cadre_spl"] };
        let mark = if has(&r["marked_at_mid_lamports"]) { &r["marked_at_mid_lamports"] } else { &r["position_marked_at_mid_lamports"] };
        let pos_s = if has(mark) { format!("{} CADRE ({} SOL at mid)", spl(n(pos) as u128), sol(n(mark) as u128)) } else { format!("{} CADRE", spl(n(pos) as u128)) };
        ([name(r), kind_of(r).into(), format!("{} SOL", signed(pnl, sol)), pos_s, n(&r["trades"]).to_string()], if pnl < 0 { RED } else if pnl > 0 { GREEN } else { TEXT })
    };
    let mut user = vec![];
    let mut house = vec![];
    let (mut uc, mut hc) = (vec![], vec![]);
    for r in board["user_agents"].as_array().into_iter().flatten() {
        let (x, c) = row(r);
        user.push(x);
        uc.push(c);
    }
    for r in board["solana"].as_array().into_iter().flatten() {
        let (x, c) = row(r);
        if kind_of(r) == "user agent" {
            user.push(x);
            uc.push(c);
        } else {
            house.push(x);
            hc.push(c);
        }
    }
    (user, house, uc, hc)
}

/// One fill in words: (when, who + kind, side + amounts, ref).
pub fn trade_line(t: &Value) -> [String; 4] {
    let (inp, out) = (n(&t["amount_in"]) as u128, n(&t["amount_out"]) as u128);
    let what = if s(t, "venue") == "pumpswap" || s(t, "chain") == "solana" {
        if s(t, "side") == "buy" {
            format!("buy {} SOL -> {} CADRE", sol(inp), spl(out))
        } else {
            format!("sell {} CADRE -> {} SOL", spl(inp), sol(out))
        }
    } else {
        format!("{} {} -> {}", s(t, "side"), inp, out)
    };
    let who = {
        let h = s(t, "handle");
        let a = if h.is_empty() { short_n(s(t, "agent"), 10) } else { format!("@{h}") };
        format!("{a} ({}{})", kind_of(t), if s(t, "via").is_empty() { String::new() } else { format!(", {}", s(t, "via")) })
    };
    let at = n(&t["at"]);
    let when = if at > 1_000_000_000_000 { super::views::hms(at as u64) } else if at > 0 { format!("h {at}") } else { String::new() };
    let mut r = short_n(s(t, "ref"), 18);
    if !s(t, "decided_by").is_empty() {
        r = format!("{r} by {}", short_n(s(t, "decided_by"), 14));
    }
    [when, who, what, r]
}

/// Your positions and balances.
pub fn mine_lines(me: Option<&str>, mine: &Option<Result<Option<Value>, String>>, bal: &Option<Result<Value, String>>) -> Vec<Line> {
    let Some(me) = me else {
        return vec![("Wallet".into(), "none: start with --key FILE or --address ADDR".into(), DIM)];
    };
    let mut l: Vec<Line> = vec![("Account".into(), format!("{} (Solana {})", short_addr(me), short_n(me, 12)), TEXT)];
    match bal {
        Some(Ok(b)) => {
            let so = &b["solana"];
            if so.is_object() && so.get("error").is_none() {
                l.push(("SOL".into(), sol_s(&so["lamports"]), TEXT));
                l.push(("CADRE (Solana)".into(), format!("{} CADRE", spl(n(&so["spl"]) as u128)), TEXT));
            } else {
                l.push(("Solana balances".into(), format!("unavailable: {}", s(so, "error")), RED));
            }
            let ch = &b["chain"];
            if ch.is_object() && has(&ch["balance"]) {
                l.push(("CADRE (Cadre chain)".into(), format!("{} CADRE", amount(n(&ch["balance"]) as u128, 9, 4)), MUTED));
            }
        }
        Some(Err(e)) => l.push(("Balances".into(), format!("unavailable: {e}"), RED)),
        None => {}
    }
    match mine {
        Some(Ok(None)) => l.push(("Trading".into(), "no trades on chain for this account".into(), DIM)),
        Some(Err(e)) => l.push(("Trading".into(), format!("unavailable: {e}"), RED)),
        Some(Ok(Some(j))) => {
            let p = &j["positions"];
            for w in p["solana_wallet"].as_array().into_iter().flatten() {
                l.push((format!("Own wallet {}", short_n(s(w, "mint"), 10)), format!("{} held, {} trades", spl(n(&w["position_spl"]) as u128), n(&w["trades"])), TEXT));
                l.push(("  Realized PnL".into(), sol_s(&w["realized_pnl_lamports"]), if n(&w["realized_pnl_lamports"]) < 0 { RED } else { GREEN }));
                if has(&w["marked_at_mid_lamports"]) {
                    l.push(("  At pool mid".into(), format!("{} (a mark, not profit)", sol_s(&w["marked_at_mid_lamports"])), DIM));
                }
            }
            for m in p["solana_mandates"].as_array().into_iter().flatten() {
                l.push((format!("Mandate {}", short_n(s(m, "mandate"), 10)), format!("{} SOL, {} CADRE", sol(n(&m["sol_equity_lamports"]) as u128), spl(n(&m["cadre_position_spl"]) as u128)), TEXT));
                l.push(("  Realized PnL".into(), sol_s(&m["realized_pnl_lamports"]), if n(&m["realized_pnl_lamports"]) < 0 { RED } else { GREEN }));
            }
            if l.len() <= 4 {
                l.push(("Positions".into(), "none open".into(), DIM));
            }
        }
        None => {}
    }
    l
}

// ---------------------------------------------------------------------------------------
// drawing

fn lines_card(c: &mut Buffer, pen: &Pen, r: Rect, title: &str, right: &str, lines: &[Line]) {
    let inner = card(c, pen, r);
    draw::title(c, pen, inner, title);
    put_right(c, inner.right(), inner.y, right, pen.card(MUTED), inner.x + str_w(title) + 2);
    for (i, (k, v, col)) in lines.iter().enumerate() {
        draw::kv(c, pen, inner, inner.y + 2 + i as u16, k, v, *col);
    }
}

pub(crate) fn table_card(c: &mut Buffer, pen: &Pen, r: Rect, title: &str, right: &str, head: &[&str], rows: &[Vec<String>], colors: &[u32], empty: &str) {
    let inner = card(c, pen, r);
    draw::title(c, pen, inner, title);
    put_right(c, inner.right(), inner.y, right, pen.card(MUTED), inner.x + str_w(title) + 2);
    if rows.is_empty() {
        put(c, inner.x, inner.y + 2, &clip(empty, inner.width as usize), pen.card(DIM), inner.right());
        return;
    }
    // column widths: content max, the last column takes the rest
    let cols = head.len();
    let mut w: Vec<u16> = head.iter().map(|h| str_w(h)).collect();
    for r in rows {
        for (i, x) in r.iter().enumerate().take(cols) {
            w[i] = w[i].max(str_w(x));
        }
    }
    let fixed: u16 = w[..cols - 1].iter().map(|x| x + 2).sum();
    if fixed + 8 > inner.width {
        let share = inner.width.saturating_sub(8) / (cols as u16 - 1).max(1);
        for x in w.iter_mut().take(cols - 1) {
            *x = (*x).min(share.saturating_sub(2)).max(4);
        }
    }
    let draw_row = |c: &mut Buffer, y: u16, cells: &[String], first: u32, rest: u32, bold: bool| {
        let mut x = inner.x;
        for (i, cell) in cells.iter().enumerate().take(cols) {
            let cw = if i == cols - 1 { inner.right().saturating_sub(x) } else { w[i] };
            let st = pen.card(if i == 2 && !bold { first } else { rest });
            put(c, x, y, &clip(cell, cw as usize), if bold { st.add_modifier(Modifier::BOLD) } else { st }, inner.right());
            x += cw + 2;
        }
    };
    draw_row(c, inner.y + 2, &head.iter().map(|h| h.to_string()).collect::<Vec<_>>(), MUTED, MUTED, false);
    for (i, r) in rows.iter().enumerate() {
        let y = inner.y + 3 + i as u16;
        if y >= inner.bottom() {
            break;
        }
        draw_row(c, y, r, colors.get(i).copied().unwrap_or(TEXT), TEXT, false);
    }
}

/// Everything on a canvas `w` wide (scrolled by the view).
pub fn trade_canvas(pen: &Pen, w: u16, d: &TradeData, me: Option<&str>) -> Buffer {
    let q = quote_lines(&d.buy, &d.sell);
    let m = mine_lines(me, &d.mine, &d.balances);
    let (users, house, uc, hc) = match &d.board {
        Ok(b) => board_rows(b),
        Err(_) => (vec![], vec![], vec![], vec![]),
    };
    let fills: Vec<Vec<String>> = match &d.trades {
        Ok(t) => t["trades"].as_array().into_iter().flatten().map(|x| trade_line(x).to_vec()).collect(),
        Err(_) => vec![],
    };
    let card_h = |n: usize| n as u16 + 5;
    let two = w >= 120;
    let top_h = card_h(q.len().max(m.len()));
    let tb_h = |n: usize| (n.max(1) as u16) + 6;
    let total = if two { top_h } else { card_h(q.len()) + card_h(m.len()) } + tb_h(users.len()) + tb_h(house.len()) + tb_h(fills.len()) + 4;
    let mut c = canvas(w, total);
    let mut y = 0;
    if two {
        let cw = (w - 2) / 2;
        lines_card(&mut c, pen, Rect::new(0, 0, cw, top_h), "CADRE on PumpSwap", "Solana", &q);
        lines_card(&mut c, pen, Rect::new(cw + 2, 0, w - cw - 2, top_h), "Your positions", "cadre buy / sell", &m);
        y += top_h;
    } else {
        lines_card(&mut c, pen, Rect::new(0, 0, w, card_h(q.len())), "CADRE on PumpSwap", "Solana", &q);
        y += card_h(q.len());
        lines_card(&mut c, pen, Rect::new(0, y, w, card_h(m.len())), "Your positions", "cadre buy / sell", &m);
        y += card_h(m.len());
    }
    let head = ["AGENT", "KIND", "REALIZED PNL", "POSITION", "TRADES"];
    let board_err = d.board.as_ref().err().map(|e| format!("leaderboard unavailable: {e}"));
    let to_rows = |r: &[[String; 5]]| r.iter().map(|x| x.to_vec()).collect::<Vec<_>>();
    table_card(&mut c, pen, Rect::new(0, y, w, tb_h(users.len())), "Leaderboard: user agents", "organic, own wallets", &head, &to_rows(&users), &uc, board_err.as_deref().unwrap_or("no user agent has traded yet"));
    y += tb_h(users.len());
    table_card(&mut c, pen, Rect::new(0, y, w, tb_h(house.len())), "Leaderboard: house traders", "labelled, excluded from organic volume", &head, &to_rows(&house), &hc, board_err.as_deref().unwrap_or("no house fills"));
    y += tb_h(house.len());
    let terr = d.trades.as_ref().err().map(|e| format!("trades unavailable: {e}"));
    let fc: Vec<u32> = fills.iter().map(|f| if f[2].starts_with("buy") { GREEN } else { YELLOW }).collect();
    table_card(&mut c, pen, Rect::new(0, y, w, tb_h(fills.len())), "Latest fills", "every fill is a Solana signature, its receipt on Cadre", &["WHEN", "WHO", "FILL", "TX"], &fills, &fc, terr.as_deref().unwrap_or("no fills on chain yet"));
    c
}

pub fn draw_trade(buf: &mut Buffer, pen: &Pen, r: Rect, st: &TradeState, me: Option<&str>, ms: u128, loading: bool) {
    let Some(d) = &st.data else {
        put(buf, r.x + 2, r.y, &format!("{} reading quotes, the leaderboard and fills from cadre-api", draw::spinner(ms)), pen.fg(MUTED), r.right());
        return;
    };
    let x = put(buf, r.x, r.y, "Trade on Solana", pen.fg(TEXT).add_modifier(Modifier::BOLD), r.right());
    let x = put(buf, x + 2, r.y, "cadre buy --sol 0.01 --key FILE  ·  cadre sell --tokens 1000 --key FILE  (a review and a y/N before signing)", pen.fg(MUTED), r.right());
    if loading {
        put_right(buf, r.right(), r.y, &format!("{} reading", draw::spinner(ms)), pen.fg(DIM), x + 2);
    }
    let body = Rect::new(r.x, r.y + 2, r.width, r.height.saturating_sub(2));
    let c = trade_canvas(pen, body.width, d, me);
    let top = st.scroll.min(max_scroll(c.area.height, body.height));
    blit(&c, buf, body, top);
    let _ = BLUE;
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn data() -> TradeData {
        TradeData {
            buy: Ok(json!({"side": "buy", "mint": "BwJqbsJUXshRj9ABxXs4RZ838kZrSqw8TT4CKMNfEu5e", "venue": "pumpswap", "tokens_out_spl": "380000000000", "reserves": {"base_spl": "80600000000000", "quote_lamports": "2120000000"}})),
            sell: Ok(json!({"side": "sell", "venue": "pumpswap", "sol_out_lamports": "2630000"})),
            board: Ok(json!({
                "solana": [{"agent": "7g93HBvUF7F1zNUahKnpxXEB3ryhBXNFS58ciX1zJxkL", "handle": "desk_maker", "house": true, "realized_pnl_lamports": "-1200000", "position_cadre_spl": "5000000", "position_marked_at_mid_lamports": "130000", "trades": 4}],
                "user_agents": [{"agent": "9xQeWvG816bUx9EPjHmaT23yvVM2ZWbrrpZb9PusVFin", "handle": "tide_trader", "name": "Tide", "creator": "H919SkKQQoS3HuJK2gNQdsCvFibBdWzW388xpi5tVQ3p", "kind": "user_agent", "realized_pnl_lamports": "300000", "position_spl": "0", "marked_at_mid_lamports": null, "trades": 2}]
            })),
            trades: Ok(json!({"trades": [{"chain": "solana", "venue": "pumpswap", "agent": "9xQeWvG816bUx9EPjHmaT23yvVM2ZWbrrpZb9PusVFin", "handle": "tide_trader", "kind": "user_agent", "via": "wallet", "side": "buy", "amount_in": "2000000", "amount_out": "75000000000", "at": 1792500000000i64, "ref": "sol:5abc", "decided_by": "req_42"}]})),
            mine: Some(Ok(Some(json!({"positions": {"solana_wallet": [{"mint": "BwJqbsJUXshRj9ABxXs4RZ838kZrSqw8TT4CKMNfEu5e", "position_spl": "75000000000", "realized_pnl_lamports": "0", "marked_at_mid_lamports": "1900000", "trades": 1}], "solana_mandates": []}})))),
            balances: Some(Ok(json!({"solana": {"lamports": "50000000", "spl": "75000000000"}, "chain": {"balance": "1000000000"}}))),
        }
    }

    #[test]
    fn quote_card_from_the_pool() {
        let d = data();
        let l = quote_lines(&d.buy, &d.sell);
        let t: Vec<String> = l.iter().map(|(a, b, _)| format!("{a}: {b}")).collect();
        assert!(t.iter().any(|x| x == "Venue: pumpswap (Solana)"), "{t:?}");
        assert!(t.iter().any(|x| x.starts_with("Buy with 0.01 SOL: 380000") || x.contains("380,000")), "{t:?}");
        assert!(t.iter().any(|x| x.starts_with("Sell 100") && x.contains("SOL before fees")), "{t:?}");
        let e = quote_lines(&Err("pool_missing".into()), &Err("x".into()));
        assert!(e[0].1.starts_with("unavailable: pool_missing"));
    }

    #[test]
    fn leaderboard_keeps_user_agents_apart_from_house() {
        let d = data();
        let (u, h, uc, hc) = board_rows(d.board.as_ref().unwrap());
        assert_eq!(u.len(), 1);
        assert_eq!(h.len(), 1);
        assert_eq!(u[0][0], "@tide_trader Tide");
        assert_eq!(u[0][1], "user agent");
        assert_eq!(h[0][1], "house");
        assert!(h[0][2].starts_with('-'));
        assert_eq!(hc[0], RED);
        assert_eq!(uc[0], GREEN);
        // a row with kind user_agent inside `solana` also counts as a user agent
        let (u2, h2, _, _) = board_rows(&json!({"solana": [{"agent": "a", "kind": "user_agent", "realized_pnl_lamports": "0"}]}));
        assert_eq!((u2.len(), h2.len()), (1, 0));
    }

    #[test]
    fn fills_say_who_decided() {
        let d = data();
        let t = &d.trades.as_ref().unwrap()["trades"][0];
        let l = trade_line(t);
        assert!(l[1].starts_with("@tide_trader (user agent, wallet)"), "{l:?}");
        assert!(l[2].starts_with("buy 0.002 SOL -> "), "{l:?}");
        assert!(l[3].contains("by req_42"));
    }

    #[test]
    fn positions_and_missing_parts() {
        let d = data();
        let l = mine_lines(Some("9xQeWvG816bUx9EPjHmaT23yvVM2ZWbrrpZb9PusVFin"), &d.mine, &d.balances);
        let t: Vec<String> = l.iter().map(|(a, b, _)| format!("{a}: {b}")).collect();
        assert!(t.iter().any(|x| x.starts_with("SOL: 0.05")), "{t:?}");
        assert!(t.iter().any(|x| x.contains("a mark, not profit")), "{t:?}");
        let none = mine_lines(None, &None, &None);
        assert!(none[0].1.contains("--key"));
        let nt = mine_lines(Some("abc"), &Some(Ok(None)), &Some(Err("timeout".into())));
        assert!(nt.iter().any(|x| x.1 == "no trades on chain for this account"));
        assert!(nt.iter().any(|x| x.1 == "unavailable: timeout"));
    }

    #[test]
    fn canvas_draws_every_section_at_both_widths() {
        let d = data();
        let pen = Pen { mode: super::super::theme::Mode::NoColor };
        for w in [90u16, 150] {
            let c = trade_canvas(&pen, w, &d, Some("9xQeWvG816bUx9EPjHmaT23yvVM2ZWbrrpZb9PusVFin"));
            let text: String = (0..c.area.height).map(|y| (0..c.area.width).map(|x| c[(x, y)].symbol().to_string()).collect::<String>() + "\n").collect();
            for want in ["CADRE on PumpSwap", "Your positions", "Leaderboard: user agents", "Leaderboard: house traders", "Latest fills", "@tide_trader", "@desk_maker"] {
                assert!(text.contains(want), "{w}: {want}\n{text}");
            }
        }
    }
}
