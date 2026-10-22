//! Agent-chain tx status (docs/SPEED.md 2.4/2.5): pending / preconfirmed (weak blocks on the
//! current tip list the tx) / included (N of K confirmations) / final. `normalize` reads a node's
//! getTxStatus or getTx answer; an older node has no `final` flag and no preconfirmed status, so
//! final is derived from the finalized height (or confirmations > K). `follow` is `cadre tx
//! --watch`: it polls the node and prints every transition with its time until the tx is final.

use std::io::Write;
use std::time::{Duration, Instant};

use anyhow::{bail, Result};
use serde_json::{json, Value};

use crate::commands::Out;
use crate::ctx::Ctx;
use crate::probe::now_ms;
use crate::term::Tone;

#[derive(Debug, Clone, PartialEq)]
pub struct Preconf {
    pub weak_count: u64,
    pub weak_miners: u64,
    pub weak_share: f64,
    pub first_seen_ms: Option<u64>,
    pub latest_age_ms: Option<u64>,
    pub confidence: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct TxState {
    /// pending | preconfirmed | included | final
    pub status: &'static str,
    pub preconf: Option<Preconf>,
    pub height: Option<u64>,
    pub confirmations: Option<u64>,
    pub finality_k: Option<u64>,
    pub is_final: bool,
    /// "node" or "derived" (from the finalized height or K) when final.
    pub final_source: Option<&'static str>,
}

fn n(v: &Value) -> Option<u64> {
    v.as_u64().or_else(|| v.as_str().and_then(|s| s.parse().ok())).or_else(|| v.as_f64().filter(|x| *x >= 0.0).map(|x| x as u64))
}

/// Confidence label from the counts (SPEED.md 2.4), when the node sent none.
pub fn confidence_of(count: u64, miners: u64, share: f64) -> &'static str {
    if count >= 4 && miners >= 2 && share >= 0.9 {
        "high"
    } else if count >= 2 && miners >= 2 {
        "medium"
    } else {
        "low"
    }
}

pub fn parse_preconf(p: &Value) -> Option<Preconf> {
    let count = n(&p["weak_count"]).filter(|c| *c >= 1)?;
    let miners = n(&p["weak_miners"]).unwrap_or(1).max(1);
    let share = p["weak_share"].as_f64().unwrap_or(0.0).clamp(0.0, 1.0);
    let confidence = match p["confidence"].as_str() {
        Some(c @ ("low" | "medium" | "high")) => c.to_string(),
        _ => confidence_of(count, miners, share).to_string(),
    };
    Some(Preconf { weak_count: count, weak_miners: miners, weak_share: share, first_seen_ms: n(&p["first_seen_ms"]), latest_age_ms: n(&p["latest_age_ms"]), confidence })
}

/// Normalize a getTxStatus / getTx answer; `info` is getChainInfo (or Null).
pub fn normalize(v: &Value, info: &Value) -> TxState {
    let preconf = parse_preconf(&v["preconf"]);
    let height = n(&v["height"]);
    let confirmations = n(&v["confirmations"]);
    let k = n(&v["finality_k"]).or_else(|| n(&info["finality_k"]));
    let raw = v["status"].as_str().unwrap_or("");
    let mut st = TxState { status: "pending", preconf: None, height: None, confirmations: None, finality_k: k, is_final: false, final_source: None };
    if raw == "final" || (v["final"] == json!(true) && height.is_some()) {
        st.status = "final";
        st.is_final = true;
        st.final_source = Some("node");
    } else if raw == "included" || (height.is_some() && raw != "pending" && raw != "preconfirmed") {
        st.status = "included";
        if v["final"] != json!(false) {
            let derived = match (height, n(&info["finalized_height"])) {
                (Some(h), Some(f)) if h <= f && confirmations != Some(0) => true,
                _ => matches!((confirmations, k), (Some(c), Some(k)) if c > k),
            };
            if derived {
                st.status = "final";
                st.is_final = true;
                st.final_source = Some("derived");
            }
        }
    } else if raw == "preconfirmed" || preconf.is_some() {
        st.status = "preconfirmed";
        st.preconf = preconf;
    }
    if matches!(st.status, "included" | "final") {
        st.height = height;
        st.confirmations = confirmations;
    }
    st
}

fn age(ms: Option<u64>) -> String {
    match ms {
        None => "unknown age".into(),
        Some(m) if m < 60_000 => format!("{:.1} s ago", m as f64 / 1000.0),
        Some(m) => format!("{} min ago", m / 60_000),
    }
}

/// One line: "preconfirmed (3 weak blocks, 2 miners, medium, latest 1.2 s ago)".
pub fn label(s: &TxState) -> String {
    match s.status {
        "preconfirmed" => match &s.preconf {
            Some(p) => format!(
                "preconfirmed ({} weak block{}, {} miner{}, {}, latest {})",
                p.weak_count,
                if p.weak_count == 1 { "" } else { "s" },
                p.weak_miners,
                if p.weak_miners == 1 { "" } else { "s" },
                p.confidence,
                age(p.latest_age_ms)
            ),
            None => "preconfirmed".into(),
        },
        "included" => format!(
            "included at {} ({}{} confirmations)",
            s.height.map(|h| h.to_string()).unwrap_or_else(|| "?".into()),
            s.confirmations.map(|c| c.to_string()).unwrap_or_else(|| "?".into()),
            s.finality_k.map(|k| format!(" of {k}")).unwrap_or_default()
        ),
        "final" => format!("final at {}", s.height.map(|h| h.to_string()).unwrap_or_else(|| "?".into())),
        _ => "pending".into(),
    }
}

/// What the status does and does not guarantee (SPEED.md 2.5), one line.
pub fn risk(s: &TxState) -> String {
    match s.status {
        "pending" => "In the mempool; no miner has committed to it yet.".into(),
        "preconfirmed" => match &s.preconf {
            Some(p) if p.confidence != "low" => format!(
                "Listed by {} weak blocks from {} miners ({:.0}% of recent weak blocks). Not binding until included: fine for small, reversible actions only.",
                p.weak_count,
                p.weak_miners,
                p.weak_share * 100.0
            ),
            _ => "Seen in one miner's weak block on the current tip. Not binding: the next block may still leave it out.".into(),
        },
        "included" => "In a block; a reorg can still undo it until it is final.".into(),
        _ => "Final: at or below the finalized height; it cannot be reorged.".into(),
    }
}

pub fn to_json(s: &TxState) -> Value {
    json!({
        "status": s.status,
        "preconf": s.preconf.as_ref().map(|p| json!({"weak_count": p.weak_count, "weak_miners": p.weak_miners, "weak_share": p.weak_share,
            "first_seen_ms": p.first_seen_ms, "latest_age_ms": p.latest_age_ms, "confidence": p.confidence})),
        "height": s.height, "confirmations": s.confirmations, "final": s.is_final, "finality_k": s.finality_k,
        "final_source": s.final_source, "risk": risk(s), "label": label(s),
    })
}

/// getTxStatus, or getTx on a node from before the weak-block upgrade. Null when unknown.
pub fn read(ctx: &Ctx, hash: &str) -> Result<Value> {
    match ctx.call("getTxStatus", json!({"tx_hash": hash})) {
        Ok(v) => Ok(v),
        Err(e) if format!("{e:#}").to_ascii_lowercase().contains("unknown method") || format!("{e:#}").to_ascii_lowercase().contains("method not found") => {
            ctx.call("getTx", json!({"tx_hash": hash}))
        }
        Err(e) => Err(e),
    }
}

/// Is `b` a later stage than `a` (or the same stage with new evidence worth printing)?
fn changed(a: &TxState, b: &TxState) -> bool {
    if a.status != b.status {
        return true;
    }
    match (a.status, &a.preconf, &b.preconf) {
        ("preconfirmed", Some(x), Some(y)) => x.weak_count != y.weak_count || x.confidence != y.confidence,
        ("included", _, _) => a.height != b.height,
        _ => false,
    }
}

/// `cadre tx HASH --watch`: poll every `interval` until final or `timeout`, printing each
/// transition (text: one line with the elapsed time; --json: one JSON line per transition).
pub fn follow(ctx: &Ctx, hash: &str, timeout: Duration, interval: Duration) -> Result<Out> {
    let hash = hash.trim().trim_start_matches("0x").to_ascii_lowercase();
    if hash.len() != 64 || !hash.chars().all(|c| c.is_ascii_hexdigit()) {
        bail!("{hash:?} is not a Cadre tx hash (64 hex)");
    }
    let json_mode = ctx.opts.json;
    let st = ctx.ui.style();
    let t0 = Instant::now();
    let start_ms = now_ms();
    let mut last: Option<TxState> = None;
    let mut transitions: Vec<Value> = Vec::new();
    let out = std::io::stdout();
    if !json_mode {
        println!("{}", st.paint(Tone::Dim, &format!("following {} on {} until final (Ctrl-C stops)", crate::fmt::short(&hash), ctx.rpc)));
    }
    loop {
        let v = read(ctx, &hash)?;
        if v.is_null() {
            if last.is_none() && t0.elapsed() > Duration::from_secs(10) {
                bail!("no tx {hash} on {} (not in the mempool, a weak block or a block of this node)", ctx.rpc);
            }
        } else {
            let info = if v["final"].is_boolean() { Value::Null } else { ctx.call("getChainInfo", json!({})).unwrap_or(Value::Null) };
            let s = normalize(&v, &info);
            if last.as_ref().is_none_or(|l| changed(l, &s)) {
                let ms = t0.elapsed().as_millis() as u64;
                let mut j = to_json(&s);
                j["elapsed_ms"] = json!(ms);
                j["at_ms"] = json!(start_ms + ms);
                j["event"] = json!("status");
                j["tx_hash"] = json!(hash);
                let mut lock = out.lock();
                if json_mode {
                    let _ = writeln!(lock, "{j}");
                } else {
                    let tone = match s.status {
                        "final" => Tone::Good,
                        "included" => Tone::Good,
                        "preconfirmed" => Tone::Warn,
                        _ => Tone::Dim,
                    };
                    let _ = writeln!(lock, "{:>8.1} s  {}  {}", ms as f64 / 1000.0, st.paint(tone, &label(&s)), st.paint(Tone::Dim, &risk(&s)));
                }
                let _ = lock.flush();
                transitions.push(j);
                last = Some(s.clone());
            }
            if s.is_final {
                let first = |stage: &str| transitions.iter().find(|t| t["status"] == stage).map(|t| t["elapsed_ms"].clone()).unwrap_or(Value::Null);
                let summary = json!({"tx_hash": hash, "status": "final", "transitions": transitions,
                    "ms_to_preconfirmed": first("preconfirmed"), "ms_to_included": first("included"), "ms_to_final": first("final")});
                return Ok(Out::new(summary, format!("{} final after {:.1} s", crate::fmt::short(&hash), t0.elapsed().as_secs_f64())));
            }
        }
        if t0.elapsed() >= timeout {
            let s = last.as_ref().map(|l| l.status).unwrap_or("unknown");
            return Ok(Out::new(json!({"tx_hash": hash, "status": s, "timed_out": true, "transitions": transitions}), format!("{} still {s} after {} s (timeout)", crate::fmt::short(&hash), timeout.as_secs())));
        }
        std::thread::sleep(interval);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{BufRead, BufReader, Read};
    use std::net::TcpListener;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::Arc;

    #[test]
    fn normalize_every_status() {
        let info = json!({"finalized_height": 70, "finality_k": 30});
        assert_eq!(normalize(&json!({"status": "pending"}), &info).status, "pending");
        let p = normalize(&json!({"status": "preconfirmed", "preconf": {"weak_count": 3, "weak_miners": 2, "weak_share": 0.8, "latest_age_ms": 1200, "confidence": "medium"}}), &info);
        assert_eq!(p.status, "preconfirmed");
        assert_eq!(label(&p), "preconfirmed (3 weak blocks, 2 miners, medium, latest 1.2 s ago)");
        assert!(risk(&p).contains("Not binding"));
        let i = normalize(&json!({"status": "included", "height": 90, "confirmations": 10}), &info);
        assert_eq!((i.status, i.height, i.is_final), ("included", Some(90), false));
        assert_eq!(label(&i), "included at 90 (10 of 30 confirmations)");
        // an older node: final derived from the finalized height, or confirmations > K
        let f = normalize(&json!({"status": "included", "height": 60, "confirmations": 41}), &info);
        assert_eq!((f.status, f.final_source), ("final", Some("derived")));
        let f2 = normalize(&json!({"status": "included", "height": 60, "confirmations": 31}), &json!({"finality_k": 30}));
        assert_eq!(f2.status, "final");
        let nf = normalize(&json!({"status": "included", "height": 60, "confirmations": 30}), &json!({"finality_k": 30}));
        assert_eq!(nf.status, "included");
        // the node's own flags win
        let n1 = normalize(&json!({"status": "included", "height": 60, "confirmations": 41, "final": false}), &info);
        assert_eq!(n1.status, "included");
        let n2 = normalize(&json!({"status": "final", "height": 60, "final": true}), &Value::Null);
        assert_eq!((n2.status, n2.final_source), ("final", Some("node")));
        assert_eq!(confidence_of(4, 1, 1.0), "low");
        assert_eq!(confidence_of(4, 2, 0.95), "high");
        assert!(parse_preconf(&json!({"weak_count": 0})).is_none());
        for s in [&p, &i, &f] {
            assert!(!label(s).contains('\u{2014}') && !risk(s).contains('\u{2014}'));
        }
    }

    /// A node whose getTxStatus walks pending -> preconfirmed -> included -> final, one step
    /// per `per` calls; `old` = a node without getTxStatus (getTx only, no final flag).
    fn scripted(old: bool) -> String {
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}", l.local_addr().unwrap());
        let calls = Arc::new(AtomicU64::new(0));
        std::thread::spawn(move || {
            for conn in l.incoming().flatten() {
                let calls = calls.clone();
                std::thread::spawn(move || {
                    let mut r = BufReader::new(conn.try_clone().unwrap());
                    let mut w = conn;
                    loop {
                        let mut first = String::new();
                        if r.read_line(&mut first).unwrap_or(0) == 0 {
                            return;
                        }
                        let mut len = 0usize;
                        let mut line = String::new();
                        loop {
                            line.clear();
                            if r.read_line(&mut line).unwrap_or(0) == 0 {
                                return;
                            }
                            if line == "\r\n" {
                                break;
                            }
                            if let Some(v) = line.to_ascii_lowercase().strip_prefix("content-length:") {
                                len = v.trim().parse().unwrap_or(0);
                            }
                        }
                        let mut body = vec![0u8; len];
                        if r.read_exact(&mut body).is_err() {
                            return;
                        }
                        let q: Value = serde_json::from_slice(&body).unwrap_or(Value::Null);
                        let m = q["method"].as_str().unwrap_or("");
                        let res = match m {
                            "getChainInfo" => Ok(json!({"height": 100, "finalized_height": 70, "finality_k": 30})),
                            "getTxStatus" if old => Err("unknown method getTxStatus"),
                            "getTxStatus" | "getTx" => {
                                let step = calls.fetch_add(1, Ordering::SeqCst) / 2;
                                Ok(match (step, old) {
                                    (0, _) => json!({"status": "pending", "final": if old { Value::Null } else { json!(false) }}),
                                    (1, false) => json!({"status": "preconfirmed", "final": false, "preconf": {"weak_count": 1, "weak_miners": 1, "weak_share": 1.0, "latest_age_ms": 400, "confidence": "low"}}),
                                    (2, false) => json!({"status": "preconfirmed", "final": false, "preconf": {"weak_count": 3, "weak_miners": 2, "weak_share": 1.0, "latest_age_ms": 100, "confidence": "medium"}}),
                                    (1..=3, _) => json!({"status": "included", "height": 95, "confirmations": 6, "final": if old { Value::Null } else { json!(false) }}),
                                    _ if old => json!({"status": "included", "height": 60, "confirmations": 41}),
                                    _ => json!({"status": "final", "height": 95, "confirmations": 31, "final": true}),
                                })
                            }
                            _ => Err("unknown method"),
                        };
                        let a = match res {
                            Ok(v) => json!({"jsonrpc": "2.0", "id": q["id"], "result": v}),
                            Err(e) => json!({"jsonrpc": "2.0", "id": q["id"], "error": {"code": -32601, "message": e}}),
                        };
                        let b = a.to_string();
                        let _ = write!(w, "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\n\r\n{}", b.len(), b);
                    }
                });
            }
        });
        url
    }

    #[test]
    fn follow_prints_each_transition_until_final() {
        let u = scripted(false);
        let c = crate::mock::ctx(&u, &u, &u, false);
        let o = follow(&c, &"ab".repeat(32), Duration::from_secs(20), Duration::from_millis(5)).unwrap();
        let st: Vec<&str> = o.json["transitions"].as_array().unwrap().iter().map(|t| t["status"].as_str().unwrap()).collect();
        assert_eq!(st, ["pending", "preconfirmed", "preconfirmed", "included", "final"]);
        assert_eq!(o.json["transitions"][2]["preconf"]["confidence"], "medium");
        assert!(o.json["ms_to_preconfirmed"].is_u64() && o.json["ms_to_final"].is_u64());
    }

    #[test]
    fn follow_on_an_old_node_derives_final() {
        let u = scripted(true);
        let c = crate::mock::ctx(&u, &u, &u, false);
        let o = follow(&c, &"cd".repeat(32), Duration::from_secs(20), Duration::from_millis(5)).unwrap();
        let st: Vec<&str> = o.json["transitions"].as_array().unwrap().iter().map(|t| t["status"].as_str().unwrap()).collect();
        assert_eq!(st, ["pending", "included", "final"]);
        assert_eq!(o.json["transitions"][2]["final_source"], "derived");
        assert!(follow(&c, "nothex", Duration::from_secs(1), Duration::from_millis(5)).is_err());
    }
}
