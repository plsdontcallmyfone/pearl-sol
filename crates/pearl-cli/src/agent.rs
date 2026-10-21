//! `pearl agent <address>`: the agent's record from the explorer's indexer
//! (`GET <explorer>/api/agent/<address>`, docs/EXPLORER.md). Money behind completed work
//! first (P and storefront SOL, self-dealing excluded), then failures, who signed, mining,
//! buying, evaluating and the agent card. Every figure comes from the explorer, which
//! computes it from chain state; this module only formats it (`--json` passes it through).

use std::time::Duration;

use anyhow::{anyhow, Context, Result};
use serde_json::Value;

use crate::ctx::{Ctx, Network};
use crate::fmt::{short, sol, thousands, tokens};
use crate::term::{Block, Panel, Table, Tone};

/// Public explorers per network profile (docs/DEVNET.md); flag, then PEARL_EXPLORER env win.
pub fn explorer_url(ctx: &Ctx, flag: &Option<String>) -> String {
    if let Some(f) = flag {
        return f.trim_end_matches('/').to_string();
    }
    // PEARL_EXPLORER, CADRE_EXPLORER and --endpoint explorer=URL land here (endpoints.rs)
    if let Some(e) = ctx.eps.get("explorer") {
        return e.to_string();
    }
    match ctx.opts.network {
        Network::Devnet => "https://explorer.64-95-12-116.sslip.io".into(),
        Network::Testbox | Network::Localnet => "http://127.0.0.1:10670".into(),
    }
}

pub fn fetch_profile(base: &str, addr: &str) -> Result<Value> {
    let url = format!("{}/api/agent/{addr}", base.trim_end_matches('/'));
    let agent = ureq::AgentBuilder::new().timeout(Duration::from_secs(15)).build();
    let r = match agent.get(&url).call() {
        Ok(r) => r,
        Err(ureq::Error::Status(code, r)) => {
            let v: Value = r.into_json().unwrap_or(Value::Null);
            return Err(anyhow!("explorer {url}: HTTP {code} {}", v.get("error").and_then(|e| e.as_str()).unwrap_or("")));
        }
        Err(e) => return Err(anyhow!("explorer {url}: {e}")),
    };
    r.into_json::<Value>().with_context(|| format!("explorer {url}: not JSON"))
}

fn st<'a>(v: &'a Value, path: &[&str]) -> &'a Value {
    let mut c = v;
    for k in path {
        c = c.get(*k).unwrap_or(&Value::Null);
    }
    c
}

fn units(v: &Value) -> u128 {
    v.as_str().and_then(|s| s.parse().ok()).or_else(|| v.as_u64().map(|x| x as u128)).unwrap_or(0)
}

fn tok(v: &Value) -> String {
    format!("{} {}", tokens(units(v)), crate::brand::TICKER)
}

fn n(v: &Value) -> String {
    v.as_u64().map(|x| thousands(x as u128)).unwrap_or_else(|| "none".into())
}

/// Parts per million as a percentage, or "none" when the figure does not exist.
pub fn pct(v: &Value) -> String {
    match v.as_u64() {
        None => "none".into(),
        Some(p) => {
            let x = p as f64 / 10_000.0;
            if x == 0.0 || x == 100.0 { format!("{x:.0}%") } else { format!("{x:.1}%") }
        }
    }
}

fn jobs(v: &Value) -> String {
    let k = v.as_u64().unwrap_or(0);
    format!("{} job{}", thousands(k as u128), if k == 1 { "" } else { "s" })
}

fn blocks(v: &Value) -> String {
    v.as_u64().map(|b| format!("{b} blocks")).unwrap_or_else(|| "none".into())
}

/// The text-mode blocks and the one-line summary of a profile.
pub fn profile_view(p: &Value) -> (Vec<Block>, String) {
    let h = st(p, &["headline"]);
    let sv = st(p, &["server"]);
    let name = st(p, &["agent", "name"]).as_str().unwrap_or("unregistered");
    let acct = p["account"].as_str().unwrap_or("");
    let line = format!(
        "{} {}: P behind completed work {}, storefront SOL {}, {} payers, accepted {} by P",
        name,
        short(acct),
        tok(&h["p_behind_completed_work"]),
        sol(units(&h["storefront_sol_behind_completed_work"])),
        n(&sv["distinct_payers"]),
        pct(&sv["acceptance_by_p_ppm"])
    );
    let sd = &sv["self_dealt_excluded"];
    let top = &sv["top_payer"];
    let rej = &sv["rejected"];
    let tally = &rej["chain_tally"];
    let mut rec = Panel::new("Agent record")
        .kv("Account", acct)
        .kv("Name", name)
        .section("Behind completed work")
        .kt("P (protocol fees)", tok(&h["p_behind_completed_work"]), Tone::Plain)
        .kt("SOL via storefront", format!("{} SOL ({} finalized order{})", sol(units(&h["storefront_sol_behind_completed_work"])), n(&h["storefront_orders"]), if h["storefront_orders"].as_u64() == Some(1) { "" } else { "s" }), Tone::Plain)
        .kv("Work pay W earned", format!("{} over {}", tok(&h["w_earned"]), jobs(&h["completed_jobs"])))
        .kv("Distinct payers", n(&sv["distinct_payers"]))
        .kv("Top payer share of P", if top.is_null() { "none".into() } else { format!("{} ({})", pct(&top["share_ppm"]), short(top["payer"].as_str().unwrap_or(""))) })
        .kv("Self-dealing excluded", format!("{} over {}, {} of completed P", tok(&sd["protocol_fees"]), jobs(&sd["jobs"]), pct(&sd["share_ppm_of_completed_p"])))
        .section("Failures and acceptance")
        .kv("Rejected by evaluators", format!("{} over {}{}", tok(&rej["protocol_fees"]), jobs(&rej["jobs"]), if tally.is_null() { String::new() } else { format!(" (chain tally {})", n(&tally["rejected"])) }))
        .kv("Delivered, then expired", format!("{} over {}", tok(&sv["delivered_then_expired"]["protocol_fees"]), jobs(&sv["delivered_then_expired"]["jobs"])))
        .kv("Accepted, by P", pct(&sv["acceptance_by_p_ppm"]))
        .kv("Signed by a third-party evaluator", format!("{} ({} of P)", tok(&sv["independent_evaluation"]["protocol_fees"]), pct(&sv["independent_evaluation"]["share_ppm"])))
        .kv("Time to completion", format!("p50 {}, p90 {}", blocks(&sv["time_to_completion_blocks"]["p50"]), blocks(&sv["time_to_completion_blocks"]["p90"])))
        .kv("Last delivery", sv["last_delivery"]["height"].as_u64().map(|x| format!("block {x}")).unwrap_or_else(|| "none".into()));
    let m = st(p, &["miner"]);
    rec = rec
        .section("Mining")
        .kv("Job blocks", format!("{}, subsidy {}", n(&m["job_blocks"]), tok(&m["subsidy"])))
        .kv("Fallback blocks", format!("{}, fees {}", n(&m["fallback_blocks"]), tok(&m["fees_fallback_blocks"])))
        .kv("Distinct jobs mined", n(&m["distinct_jobs"]));
    let b = st(p, &["buyer"]);
    rec = rec
        .section("Buying")
        .kv("Posted", format!("{} P over {}", tok(&b["protocol_fees"]), jobs(&b["jobs"])))
        .kv("Expired without a delivery", format!("{} ({} of posted P)", tok(&b["expired_without_delivery_p"]), pct(&b["expired_without_delivery_share_ppm"])));
    let e = st(p, &["evaluator"]);
    rec = rec
        .section("Evaluating")
        .kv("Completions signed", format!("{} over {}", tok(&e["protocol_fees"]), jobs(&e["jobs_signed"])))
        .kv("Distinct sellers / buyers", format!("{} / {}", n(&e["distinct_sellers"]), n(&e["distinct_buyers"])))
        .kv("Rejections signed", format!("{} over {}", tok(&e["rejects_signed_p"]), jobs(&e["rejects_signed"])))
        .kv("On its own jobs", pct(&e["own_job_share_ppm"]));
    let c = st(p, &["card"]);
    let health = c["health"].as_str().unwrap_or("none");
    rec = rec.section("Agent card").kt(
        "Health",
        health.to_string(),
        match health {
            "ok" => Tone::Good,
            "failing" | "stale" => Tone::Warn,
            _ => Tone::Dim,
        },
    );
    if let Some(u) = c["uri"].as_str().filter(|u| !u.is_empty()) {
        rec = rec.kv("URI", u);
    }
    if let Some(cn) = c["card"]["url"].as_str() {
        rec = rec.kv("Endpoint", cn);
    }
    rec = rec.kv("Listed", if c["listed"].as_bool() == Some(true) { "yes" } else { "no (stale)" });

    let mut t = Table { chip: Some("Completed work and who signed it".into()), head: vec!["Job".into(), "Payer".into(), "P".into(), "Signed by".into()], right: vec![false, false, true, false], rows: vec![], empty: "no completed work yet".into() };
    for j in sv["completed"].as_array().into_iter().flatten().take(20) {
        let signers = match j["signers"].as_array() {
            None => "not indexed".to_string(),
            Some(l) if l.is_empty() => "none".into(),
            Some(l) => l
                .iter()
                .map(|k| {
                    let k = k.as_str().unwrap_or("");
                    let role = if Some(k) == j["poster"].as_str() { " (buyer)" } else if k == acct { " (seller)" } else { "" };
                    format!("{}{role}", short(k))
                })
                .collect::<Vec<_>>()
                .join(", "),
        };
        let excluded = !j["self_dealt"].is_null();
        t.rows.push(vec![
            (format!("{}{}", short(j["job_id"].as_str().unwrap_or("")), if excluded { " excluded" } else { "" }), if excluded { Tone::Warn } else { Tone::Plain }),
            (short(j["poster"].as_str().unwrap_or("")), Tone::Plain),
            (tokens(units(&j["protocol_fee"])), Tone::Plain),
            (signers, if j["signed_by_third_party"].as_bool() == Some(true) { Tone::Good } else { Tone::Warn }),
        ]);
    }
    let mut out = vec![Block::Panel(rec), Block::Table(t)];
    out.push(Block::Hint("every figure comes from chain state via the explorer's indexer; sources: --json .sources".into()));
    (out, line)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::io::{Read, Write};

    fn sample() -> Value {
        json!({
            "account": "mFr2Tyxh8teC4gXJC3j8RzVJKVgXjTPHApJjFmjvAro",
            "agent": {"name": "summarizer-7", "uri": "https://x/card.json"},
            "headline": {"p_behind_completed_work": "7500000000", "storefront_sol_behind_completed_work": "111000000", "storefront_orders": 1, "w_earned": "37000000000", "completed_jobs": 3},
            "server": {
                "distinct_payers": 3, "top_payer": {"payer": "v1i7RKwAzZBw", "share_ppm": 533333},
                "self_dealt_excluded": {"jobs": 1, "protocol_fees": "3000000000", "share_ppm_of_completed_p": 285714},
                "rejected": {"jobs": 0, "protocol_fees": "0", "chain_tally": {"rejected": 0}},
                "delivered_then_expired": {"jobs": 1, "protocol_fees": "800000000"},
                "acceptance_by_p_ppm": 929078,
                "independent_evaluation": {"protocol_fees": "5000000000", "share_ppm": 666666},
                "time_to_completion_blocks": {"p50": 7, "p90": 10}, "last_delivery": {"height": 11},
                "completed": [{"job_id": "b2b2b2", "poster": "BUYER2", "protocol_fee": "2500000000", "signers": ["BUYER2"], "signed_by_third_party": false, "self_dealt": null}]
            },
            "miner": {"job_blocks": 1, "subsidy": "640000000", "fallback_blocks": 5, "fees_fallback_blocks": "5000", "distinct_jobs": 1},
            "buyer": {"jobs": 0, "protocol_fees": "0", "expired_without_delivery_p": "0", "expired_without_delivery_share_ppm": null},
            "evaluator": {"jobs_signed": 0, "protocol_fees": "0", "distinct_sellers": 0, "distinct_buyers": 0, "rejects_signed": 0, "rejects_signed_p": "0", "own_job_share_ppm": null},
            "card": {"health": "ok", "uri": "https://x/card.json", "listed": true, "card": {"url": "https://x/a2a"}}
        })
    }

    #[test]
    fn view_leads_with_money_and_shows_signers() {
        let (blocks, line) = profile_view(&sample());
        assert!(line.contains("P behind completed work 7.5 CADRE"), "{line}");
        assert!(line.contains("accepted 92.9% by P"), "{line}");
        let txt = crate::term::View(blocks).render(&crate::term::Style::plain());
        let i_p = txt.find("P (protocol fees)").unwrap();
        assert!(txt.find("Distinct payers").unwrap() > i_p, "money first");
        assert!(txt.contains("BUYER2 (buyer)"), "{txt}");
        assert!(txt.contains("Health"));
        assert!(!txt.contains('\u{2014}'), "no em dashes");
        assert_eq!(pct(&json!(null)), "none");
        assert_eq!(pct(&json!(1_000_000)), "100%");
    }

    #[test]
    fn fetches_api_agent_from_the_explorer() {
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = l.local_addr().unwrap().port();
        let body = sample().to_string();
        let h = std::thread::spawn(move || {
            for code in [200, 400] {
                let (mut s, _) = l.accept().unwrap();
                let mut buf = [0u8; 2048];
                let k = s.read(&mut buf).unwrap();
                let req = String::from_utf8_lossy(&buf[..k]).to_string();
                let b = if code == 200 { body.clone() } else { r#"{"error":"not a base58 address"}"#.to_string() };
                let _ = write!(s, "HTTP/1.1 {code} X\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{b}", b.len());
                if code == 200 {
                    assert!(req.starts_with("GET /api/agent/abc "), "{req}");
                }
            }
        });
        let v = fetch_profile(&format!("http://127.0.0.1:{port}/"), "abc").unwrap();
        assert_eq!(v["agent"]["name"], "summarizer-7");
        let e = fetch_profile(&format!("http://127.0.0.1:{port}"), "abc").unwrap_err().to_string();
        assert!(e.contains("HTTP 400") && e.contains("base58"), "{e}");
        h.join().unwrap();
    }
}
