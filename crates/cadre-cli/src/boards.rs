//! Leaderboards (docs/ECOSYSTEM.md Phase 5, docs/CLI.md "Boards"): `cadre top [board]` and the
//! formatting the Boards view (view 10) shares. Everything comes from cadre-api's
//! /v1/leaderboards, which ranks on chain data only: organic money (self-dealing excluded) as
//! the value, house money apart, rows below the board's minimum sample counted but not ranked.

use anyhow::{anyhow, Result};
use serde_json::{json, Value};

use crate::commands::Out;
use crate::ctx::Ctx;
use crate::fmt::{amount, short_addr};
use crate::probe::get;
use crate::term::{Block, Panel, Table, Tone, View};

pub const WINDOWS: [&str; 3] = ["24h", "7d", "all"];
pub const SIDES: [&str; 3] = ["organic", "house", "all"];

pub fn n(v: &Value) -> i128 {
    v.as_str().and_then(|x| x.parse().ok()).or_else(|| v.as_i64().map(|x| x as i128)).or_else(|| v.as_u64().map(|x| x as i128)).unwrap_or(0)
}
fn s<'a>(v: &'a Value, k: &str) -> &'a str {
    v.get(k).and_then(|x| x.as_str()).unwrap_or("")
}
fn signed(x: i128, f: impl Fn(u128) -> String) -> String {
    if x < 0 {
        format!("-{}", f((-x) as u128))
    } else {
        f(x as u128)
    }
}

/// A board value in its unit: lamports as SOL, CADRE base units (9 dp) as CADRE, ppm as a
/// percentage, counts (rows, paid followers, paid blocks, verdicts) as integers.
pub fn fmt_value(v: &Value, unit: &str) -> String {
    let x = n(v);
    match unit {
        "lamports" => format!("{} SOL", signed(x, |a| amount(a, 9, 9))),
        u if u.starts_with("CADRE base units") => format!("{} CADRE", signed(x, |a| amount(a, 9, 2))),
        u if u.contains("ppm") => format!("{:.1}%", x as f64 / 10_000.0),
        "rows" => format!("{} rows", signed(x, |a| amount(a, 0, 0))),
        u => format!("{} {}", signed(x, |a| amount(a, 0, 0)), u),
    }
}

/// @handle when the account has one, else the short Cx address.
pub fn who(row: &Value) -> String {
    let a = &row["account"];
    let h = s(a, "handle");
    if !h.is_empty() {
        format!("@{h}")
    } else {
        short_addr(s(a, "account"))
    }
}

/// One row as cells: rank, who, value, house value, sample, distinct payers.
pub fn row_cells(board: &Value, r: &Value) -> [String; 6] {
    let unit = s(board, "unit");
    let house = r["house"] == true || r["account"]["house"] == true;
    let hv = n(&r["house_value"]);
    [
        n(&r["rank"]).to_string(),
        format!("{}{}", who(r), if house { " (house)" } else { "" }),
        fmt_value(&r["value"], unit),
        if hv != 0 { fmt_value(&r["house_value"], unit) } else { String::new() },
        format!("{} {}", n(&r["sample"]), s(board, "sample_unit")),
        n(&r["distinct_payers"]).to_string(),
    ]
}

/// The line under a board: its minimum sample and what was left out.
pub fn foot(board: &Value) -> String {
    let below = n(&board["below_min"]);
    let mut t = format!("min {} {}", n(&board["min_sample"]), s(board, "sample_unit"));
    if n(&board["min_payers"]) > 0 {
        t.push_str(&format!(" from {} payers", n(&board["min_payers"])));
    }
    if below > 0 {
        t.push_str(&format!(", {below} below the minimum (counted, not ranked)"));
    }
    let ex = n(&board["totals"]["excluded_items"]);
    if ex > 0 {
        t.push_str(&format!(", {ex} self-dealt items excluded"));
    }
    t
}

pub fn check_window(w: &str) -> Result<()> {
    if WINDOWS.contains(&w) { Ok(()) } else { Err(anyhow!("window is 24h, 7d or all")) }
}
pub fn check_side(x: &str) -> Result<()> {
    if SIDES.contains(&x) { Ok(()) } else { Err(anyhow!("side is organic, house or all")) }
}

fn board_table(b: &Value, limit: usize) -> Table {
    let rows: Vec<Vec<(String, Tone)>> = b["rows"].as_array().into_iter().flatten().take(limit).map(|r| {
        let c = row_cells(b, r);
        let neg = n(&r["value"]) < 0;
        vec![
            (c[0].clone(), Tone::Dim),
            (c[1].clone(), if r["house"] == true { Tone::Dim } else { Tone::Plain }),
            (c[2].clone(), if neg { Tone::Bad } else { Tone::Good }),
            (c[3].clone(), Tone::Dim),
            (c[4].clone(), Tone::Dim),
            (c[5].clone(), Tone::Plain),
        ]
    }).collect();
    Table {
        chip: Some(format!("{} ({}, {})", s(b, "title"), s(b, "window"), s(b, "side"))),
        head: ["#", "WHO", "VALUE", "HOUSE", "SAMPLE", "PAYERS"].map(String::from).to_vec(),
        right: vec![true, false, true, true, true, true],
        rows,
        empty: "no row reaches the minimum sample in this window".into(),
    }
}

/// `cadre top [board] [--window 24h|7d|all] [--side organic|house|all] [--limit N]`.
pub fn top(ctx: &Ctx, board: Option<&str>, window: &str, side: &str, limit: usize) -> Result<Out> {
    check_window(window)?;
    check_side(side)?;
    let api = ctx.eps.get("api").ok_or_else(|| anyhow!("needs cadre-api (set CADRE_API)"))?.to_string();
    let limit = limit.clamp(1, 200);
    match board {
        Some(id) => {
            let h = get(&format!("{api}/leaderboards/{id}?window={window}&side={side}&limit={limit}"));
            if !h.ok() {
                return Err(anyhow!("cadre-api /leaderboards/{id}: HTTP {} {}", h.status, h.json["error"]["message"].as_str().unwrap_or("")));
            }
            let b = h.json;
            let p = Panel::new(s(&b, "title")).kv("Ranks", s(&b, "metric")).kv("Rule", s(&b, "rule")).kv("Sample", foot(&b));
            let view = View::panel(p).with(Block::Table(board_table(&b, limit))).hint("value = organic money only; house money is the HOUSE column, never inside VALUE; every row's refs link its txs (--json)");
            let text = format!("{}: {} rows", s(&b, "title"), b["rows"].as_array().map(|r| r.len()).unwrap_or(0));
            Ok(Out::new(b, text).view(view))
        }
        None => {
            let h = get(&format!("{api}/leaderboards?window={window}&side={side}&limit=3"));
            if !h.ok() {
                return Err(anyhow!("cadre-api /leaderboards: HTTP {} {}", h.status, h.json["error"]["message"].as_str().unwrap_or("")));
            }
            let j = h.json;
            let boards = j["boards"].as_array().cloned().unwrap_or_default();
            let mut view = View::default();
            for b in &boards {
                view = view.with(Block::Table(board_table(b, 3)));
            }
            view = view.hint(format!("cadre top <board> for more rows; boards: {}", boards.iter().map(|b| s(b, "id")).collect::<Vec<_>>().join(", ")));
            Ok(Out::new(json!({"window": j["window"], "side": j["side"], "boards": boards}), format!("{} boards", boards.len())).view(view))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> Value {
        serde_json::from_str(include_str!("../tests/fixtures/leaderboards.json")).unwrap()
    }

    #[test]
    fn values_in_their_units() {
        assert_eq!(fmt_value(&json!("57920"), "lamports"), "0.00005792 SOL");
        assert_eq!(fmt_value(&json!("-1500000000"), "lamports"), "-1.5 SOL");
        assert!(fmt_value(&json!("9148670616916000"), "CADRE base units (9 dp)").ends_with(" CADRE"));
        assert_eq!(fmt_value(&json!("666666"), "outcomes accepted (ppm)"), "66.7%");
        assert_eq!(fmt_value(&json!("1000000"), "outcomes accepted (ppm)"), "100.0%");
        assert_eq!(fmt_value(&json!("470"), "paid blocks"), "470 paid blocks");
        assert_eq!(fmt_value(&json!("13"), "verdicts"), "13 verdicts");
    }

    #[test]
    fn live_fixture_parses_into_rows() {
        let j = fixture();
        let boards = j["boards"].as_array().unwrap();
        assert_eq!(boards.len(), 9);
        let ids: Vec<&str> = boards.iter().map(|b| s(b, "id")).collect();
        for want in ["trading_pnl", "market_earnings", "acceptance_rate", "paid_dms", "hardest_thinkers", "most_tipped", "most_followed", "miners", "evaluators"] {
            assert!(ids.contains(&want), "{want}");
        }
        for b in boards {
            for r in b["rows"].as_array().unwrap() {
                let c = row_cells(b, r);
                assert!(!c[1].is_empty() && !c[2].is_empty());
                // an account without a handle prints as a short Cx address
                if r["account"]["handle"].is_null() {
                    assert!(c[1].starts_with("Cx"), "{c:?}");
                }
            }
            assert!(foot(b).starts_with("min "));
        }
        let t = &boards[0];
        assert_eq!(s(t, "id"), "trading_pnl");
        assert_eq!(row_cells(t, &t["rows"][0])[1], "@tide_ledger");
    }

    #[test]
    fn bad_window_and_side() {
        assert!(check_window("1y").is_err());
        assert!(check_side("everyone").is_err());
        assert!(check_window("24h").is_ok() && check_side("house").is_ok());
    }
}
