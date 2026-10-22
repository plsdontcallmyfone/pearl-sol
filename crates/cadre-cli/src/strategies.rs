//! Practice strategies with post-mortems (docs/STRATEGIES.md, docs/CLI.md "Strategies"):
//! `cadre strategies` lists every line (an agent's strategy and its revisions) with each version's
//! record and its knowledge record; `cadre strategies <version>` shows one version and its trades;
//! `cadre strategies --trade <entry>` one trade with its post-mortem. Everything comes from
//! cadre-api's /v1/strategies, which reads the strategies contract's own counters.

use anyhow::{anyhow, Result};
use serde_json::{json, Value};

use crate::boards::n;
use crate::commands::Out;
use crate::ctx::Ctx;
use crate::fmt::{amount, short_addr};
use crate::probe::get;
use crate::term::{Block, Panel, Table, Tone, View};

fn s<'a>(v: &'a Value, k: &str) -> &'a str {
    v.get(k).and_then(|x| x.as_str()).unwrap_or("")
}

/// Practice PnL in lamports as signed SOL.
pub fn sol(v: &Value) -> String {
    let x = n(v);
    let a = amount(x.unsigned_abs(), 9, 4);
    if x < 0 { format!("-{a} SOL") } else { format!("{a} SOL") }
}

pub fn who(v: &Value) -> String {
    let h = s(v, "handle");
    let base = if h.is_empty() { short_addr(s(v, "author")) } else { format!("@{h}") };
    if v["house"] == true { format!("{base} (house)") } else { base }
}

/// The knowledge record in one line: learned, not learned, or why it is not scored yet.
pub fn knowledge_line(k: &Value) -> String {
    if k.is_null() {
        return String::new();
    }
    let st = s(k, "status");
    match st {
        "learned" | "not_learned" => format!(
            "{} {}: {}/{} -> {}/{} (p {:.3})",
            if st == "learned" { "learned" } else { "not learned" },
            s(k, "claimed"),
            n(&k["baseline"]["faults"]),
            n(&k["window"]),
            n(&k["forward"]["faults"]),
            n(&k["window"]),
            k["fisher_p_one_sided"].as_f64().unwrap_or(1.0)
        ),
        "not_enough_trades" => format!("{}: not enough trades yet ({} of {})", s(k, "claimed"), n(&k["forward_all"]["trades"]), n(&k["min_trades"])),
        "baseline_too_small" => format!("{}: baseline too small", s(k, "claimed")),
        x => x.to_string(),
    }
}

fn record_cells(v: &Value) -> [String; 6] {
    let r = &v["record"];
    let wr = r["win_rate"].as_f64().map(|x| format!("{:.0}%", x * 100.0)).unwrap_or_else(|| "-".into());
    [
        format!("v{}{}", n(&v["id"]), if v["head"] == true { " head" } else { "" }),
        if n(&v["parent"]) > 0 { format!("v{}", n(&v["parent"])) } else { String::new() },
        format!("{} ({}W {}L {}nf)", n(&r["trades"]), n(&r["wins"]), n(&r["losses"]), n(&r["no_fills"])),
        wr,
        sol(&r["pnl_lamports"]),
        knowledge_line(&v["knowledge"]),
    ]
}

fn lines_view(j: &Value) -> View {
    let mut view = View::panel(
        Panel::new("Strategies (practice)")
            .kv("Contract", short_addr(s(j, "contract")))
            .kv("Versions", n(&j["counts"]["versions"]).to_string())
            .kv("Entries", format!("{} ({} explained, {} open)", n(&j["counts"]["entries"]), n(&j["counts"]["explained"]), n(&j["counts"]["open"])))
            .kv("Mode", "pretend balances, real Solana mainnet prices, every step committed on Cadre"),
    );
    for l in j["lines"].as_array().into_iter().flatten() {
        let rows = l["history"].as_array().into_iter().flatten().map(|v| {
            let c = record_cells(v);
            let neg = n(&v["record"]["pnl_lamports"]) < 0;
            let learned = v["knowledge"]["status"] == "learned";
            vec![
                (c[0].clone(), Tone::Plain),
                (c[1].clone(), Tone::Dim),
                (c[2].clone(), Tone::Plain),
                (c[3].clone(), Tone::Plain),
                (c[4].clone(), if neg { Tone::Bad } else { Tone::Good }),
                (c[5].clone(), if learned { Tone::Good } else { Tone::Dim }),
            ]
        }).collect();
        view = view.with(Block::Table(Table {
            chip: Some(format!("{} line {}: {} total", who(l), n(&l["line"]), sol(&l["totals"]["pnl_lamports"]))),
            head: ["VERSION", "PARENT", "TRADES", "WIN", "PNL", "KNOWLEDGE"].map(String::from).to_vec(),
            right: vec![false, false, true, true, true, false],
            rows,
            empty: "no versions".into(),
        }));
    }
    view.hint("cadre strategies <version> for its trades; --trade <entry> for one post-mortem; a fix counts as learned only on 10+ forward trades")
}

fn trades_table(ts: &[Value]) -> Table {
    let rows = ts.iter().map(|t| {
        let pm = &t["postmortem"];
        let out = if t["exit"].is_null() { "open".to_string() } else if pm.is_null() { "exited".into() } else { s(pm, "outcome").to_string() };
        let buckets = pm["names"].as_array().map(|a| a.iter().filter_map(|x| x.as_str()).collect::<Vec<_>>().join(" ")).unwrap_or_default();
        let neg = n(&t["pnl_lamports"]) < 0;
        vec![
            (format!("#{}", n(&t["id"])), Tone::Dim),
            (out.clone(), match out.as_str() { "win" => Tone::Good, "loss" => Tone::Bad, "open" | "exited" => Tone::Warn, _ => Tone::Plain }),
            (if t["exit"].is_null() { String::new() } else { s(&t["exit"], "reason").to_string() }, Tone::Plain),
            (if t["pnl_lamports"].is_null() { String::new() } else { sol(&t["pnl_lamports"]) }, if neg { Tone::Bad } else { Tone::Plain }),
            (buckets, Tone::Plain),
            (n(&t["height"]).to_string(), Tone::Dim),
        ]
    }).collect();
    Table {
        chip: Some("Trades".into()),
        head: ["ENTRY", "OUTCOME", "EXIT", "PNL", "BUCKETS", "HEIGHT"].map(String::from).to_vec(),
        right: vec![false, false, false, true, false, true],
        rows,
        empty: "no trades yet".into(),
    }
}

/// `cadre strategies [version] [--trade N]`.
pub fn strategies(ctx: &Ctx, version: Option<u64>, trade: Option<u64>) -> Result<Out> {
    let api = ctx.eps.get("api").ok_or_else(|| anyhow!("needs cadre-api (set CADRE_API)"))?.to_string();
    let fetch = |p: &str| -> Result<Value> {
        let h = get(&format!("{api}{p}"));
        if !h.ok() {
            return Err(anyhow!("cadre-api {p}: HTTP {} {}", h.status, h.json["error"]["message"].as_str().unwrap_or("")));
        }
        Ok(h.json)
    };
    if let Some(id) = trade {
        let j = fetch(&format!("/strategies/trades/{id}"))?;
        let t = &j["trade"];
        let pm = &j["blobs"]["postmortem"];
        let mut p = Panel::new(&format!("Trade #{id}"))
            .kv("Agent", who(t))
            .kv("Version", format!("v{}", n(&t["version"])))
            .kv("Size", sol(&t["size"]))
            .kv("Entry height", n(&t["height"]).to_string());
        if !t["exit"].is_null() {
            p = p.kv("Exit", format!("{} at height {}", s(&t["exit"], "reason"), n(&t["exit"]["height"]))).kv("PnL", sol(&t["exit"]["pnl"]));
        }
        if !t["postmortem"].is_null() {
            let names = t["postmortem"]["names"].as_array().map(|a| a.iter().filter_map(|x| x.as_str()).collect::<Vec<_>>().join(", ")).unwrap_or_default();
            p = p.kv("Outcome", s(&t["postmortem"], "outcome")).kv("Buckets", if names.is_empty() { "none".into() } else { names });
        }
        if !pm.is_null() {
            if let Some(l) = pm["lesson"].as_str().filter(|x| !x.is_empty()) {
                p = p.kv("Lesson", l);
            }
            for v in pm["verdicts"].as_array().into_iter().flatten().filter(|v| v["fault"] == true) {
                let ev = v["evidence"].as_array().and_then(|e| e.first()).and_then(|x| x.as_str()).unwrap_or("");
                p = p.kv(s(v, "bucket"), ev);
            }
        }
        let text = format!("trade #{id}: {}", if t["postmortem"].is_null() { "not explained yet" } else { s(&t["postmortem"], "outcome") });
        return Ok(Out::new(j.clone(), text).view(View::panel(p).hint("--json for the full snapshot, trade record and post-mortem blobs")));
    }
    if let Some(id) = version {
        let j = fetch(&format!("/strategies/versions/{id}"))?;
        let v = &j["version"];
        let mut p = Panel::new(&format!("Version {id}"))
            .kv("Agent", who(v))
            .kv("Line", n(&v["line"]).to_string())
            .kv("Hash", s(v, "hash"))
            .kv("Committed", format!("height {}", n(&v["height"])))
            .kv("Spec", if j["spec_verified"] == true { "blob matches the committed hash" } else { "blob not held" });
        if let Some(c) = j["spec"]["change"].as_str().filter(|x| !x.is_empty()) {
            p = p.kv("Change", c);
        }
        if !j["knowledge"].is_null() {
            p = p.kv("Knowledge", knowledge_line(&j["knowledge"]));
        }
        let ts: Vec<Value> = j["trades"].as_array().cloned().unwrap_or_default();
        let text = format!("version {id}: {} trades", ts.len());
        return Ok(Out::new(j.clone(), text).view(View::panel(p).with(Block::Table(trades_table(&ts)))));
    }
    let j = fetch("/strategies")?;
    let text = format!("{} strategy lines, {} versions", j["lines"].as_array().map(|x| x.len()).unwrap_or(0), n(&j["counts"]["versions"]));
    Ok(Out::new(json!(j), text).view(lines_view(&j)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn knowledge_lines() {
        let k = json!({"status": "learned", "claimed": "RISK_MANAGEMENT", "window": 10, "baseline": {"faults": 6}, "forward": {"faults": 2}, "fisher_p_one_sided": 0.085});
        assert_eq!(knowledge_line(&k), "learned RISK_MANAGEMENT: 6/10 -> 2/10 (p 0.085)");
        let w = json!({"status": "not_enough_trades", "claimed": "THESIS", "forward_all": {"trades": 3}, "min_trades": 10});
        assert_eq!(knowledge_line(&w), "THESIS: not enough trades yet (3 of 10)");
        assert_eq!(knowledge_line(&Value::Null), "");
    }

    #[test]
    fn pnl_and_names() {
        assert_eq!(sol(&json!("-2000000")), "-0.002 SOL");
        assert_eq!(sol(&json!("150000000")), "0.15 SOL");
        assert_eq!(who(&json!({"handle": "tape_lark", "house": true, "author": "x"})), "@tape_lark (house)");
        let c = record_cells(&json!({"id": 2, "parent": 1, "head": true, "record": {"trades": 10, "wins": 8, "losses": 2, "no_fills": 0, "win_rate": 0.8, "pnl_lamports": "6000000"}, "knowledge": null}));
        assert_eq!(c[0], "v2 head");
        assert_eq!(c[1], "v1");
        assert_eq!(c[3], "80%");
    }
}
