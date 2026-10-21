//! Tests only: one loopback HTTP server that plays a whole Cadre network for `status`,
//! `ping`, `tx`, `account`, `block`, `agent`, `agents`, `ticket`, `verify` and `watch`:
//! the agent-chain node JSON-RPC and the Solana JSON-RPC on POST, and cadre-api, the
//! explorer, storefront-x402, the relayer and the site on GET. Knobs make one node fork,
//! the relayer fail or a node go away, so the FAIL paths and exit codes are tested too.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use std::sync::Arc;

use base64::Engine as _;
use serde_json::{json, Value};

use agent_types::social::Post;
use agent_types::tx::{Tx, TxBody};

use crate::ctx::{Ctx, Opts};
use crate::term::{Style, Ui};

pub const PAYER: &str = "XQUUCs5tZaJrf4VMuKJRmUbA2gEV2M7uKWTQiSTe6Cy";
pub const AGENT: &str = "FP8P7NqbeHDdBZN8mz6JqXSHmMhEB44M1ETEyaeUezX4";
pub const TICKET: &str = "ecb7a659f7e4f1f0143b79068906fcd01df4c5eaa41ae6339943d4f8e20149e0";
pub const SOL_SIG: &str = "3XhAW5JYV4ajJTPyXg4Q4Vay2spS1tBkopeGVdvRduAc45yF1kQdJMEuyHrR5Zkpq31wqFEqUudZLTUfLfJMzw41";

#[derive(Clone, Default)]
pub struct Knobs {
    /// This node reports another finalized block at the common height.
    pub fork: bool,
    /// The relayer's /health says ok: false, last OK an hour ago.
    pub relayer_down: bool,
    /// cadre-api's summary without its Solana parts (as when Solana answers 429).
    pub thin_summary: bool,
}

fn now_ms() -> u64 {
    crate::probe::now_ms()
}

fn b64(b: &[u8]) -> String {
    base64::engine::general_purpose::STANDARD.encode(b)
}

pub fn post_tx() -> Tx {
    let p = Post { kind: 0, parent: [0; 32], channel: [0; 32], body: b"The tide is in.".to_vec(), blob_uri: vec![], blob_hash: [0; 32], thought_ref: b"req_ed20b1fb25b612d18d75a3b8".to_vec() };
    Tx { chain_id: 7, from: crate::solana::pk(AGENT), nonce: 1, fee: 1000, body: TxBody::Post(p), sig: [0; 64] }
}

fn bridge_config_bytes() -> Vec<u8> {
    let mut d = vec![0u8; 8];
    d.extend([1u8; 32]); // admin
    d.extend(crate::solana::pk(AGENT)); // mint (any key)
    d.extend([7u8; 32]); // vault
    d.extend(crate::solana::pk(crate::solana::TOKEN_2022_PROGRAM));
    d.extend(0u64.to_le_bytes()); // deposit_nonce: no deposits in the window
    d.push(1); // threshold
    d.push(254); // bump
    d.extend(1u32.to_le_bytes());
    d.extend([3u8; 32]);
    d.push(2); // release modes
    d.extend([4u8; 32]);
    d
}

fn rpc_answer(k: &Knobs, method: &str, p: &Value) -> Result<Value, String> {
    let tip = 100u64;
    let header = |h: u64| {
        let hash = if k.fork && h == 70 { "ee".repeat(32) } else { format!("{:064x}", h) };
        json!({"hash": hash, "canonical": true, "final": h <= 70, "confirmations": tip - h,
               "header_fields": {"height": h, "timestamp_ms": now_ms() - (tip - h) * 60_000, "job_id": if h % 2 == 0 { "ab".repeat(32) } else { "00".repeat(32) }, "miner": AGENT}})
    };
    Ok(match method {
        // agent chain
        "getChainInfo" => json!({"height": tip, "finalized_height": 70, "finalized_hash": "00", "peers": 2, "syncing": false, "mempool_size": 0, "chain_id": 7,
                                 "genesis_hash": "11".repeat(32), "tip": format!("{:064x}", tip), "timestamp_ms": now_ms() - 20_000, "finality_k": 30}),
        "getHeader" => header(p["height"].as_u64().unwrap_or(0)),
        "getBlock" => {
            let h = p["height"].as_u64().unwrap_or(tip);
            let mut b = header(h);
            let t = post_tx();
            b["txs"] = json!([{"tx_hash": hex::encode(t.hash()), "kind": "Post", "from": AGENT, "nonce": 1, "fee": "1000", "tx": b64(&t.encode())}]);
            b["reward"] = json!("84000000000");
            b["fees"] = json!("1000");
            b["subsidy"] = json!("83999999000");
            b
        }
        "getTx" => {
            let t = post_tx();
            json!({"tx_hash": p["tx_hash"], "status": "included", "height": 90, "index": 0, "confirmations": 10, "kind": "Post", "from": AGENT, "nonce": 1, "fee": "1000", "tx": b64(&t.encode())})
        }
        "getParams" => json!({"min_fee": "1000", "keys": {"min_fee": 1}}),
        "getEscrow" => json!({"escrow_balance": "999999995000000000", "outside_escrow": "5000000000", "total_supply": "1000000000000000000", "pool_balance": "0"}),
        "getBridgeEvents" => json!({"withdrawal_count": 0, "deposit_count": 3, "withdrawals": []}),
        "getProfile" | "getHandle" => json!({"account": AGENT, "has_profile": true, "handle": "ferry_nan", "name": "Old Nan", "is_agent": true, "house": true,
                                             "operator": PAYER, "operator_linked": true, "stats": {"posts": 1, "replies": 0, "followers": 0, "following": 0}}),
        "getAccount" => json!({"account": AGENT, "balance": "49998000", "nonce": 2, "route": null}),
        "getAgent" | "getRoute" => Value::Null,
        "getAccountTxs" => {
            let t = post_tx();
            json!({"txs": [{"tx_hash": hex::encode(t.hash()), "kind": "Post", "from": AGENT, "height": 90, "role": "sender", "fee": "1000", "tx": b64(&t.encode()), "final": true}]})
        }
        "getJob" => json!({"job_id": p["job_id"], "kind": "ticket", "status": if p["job_id"] == TICKET { "completed" } else { "open" }, "closed_height": 60, "result_uri": format!("blob:{}", "88".repeat(32)),
                           "evaluators": ["9U5wdELZRod6PGxczjyLn91meTHuzVLAoEnFywczC5VD"], "reject_count": 0, "standing": {"rows": 255, "rows_used": 48}}),
        "getJobRejects" => json!({"rejects": []}),
        "getBlob" => json!({"blob_id": p["blob_id"], "data": b64(json!({"format": "cadre-response-v1", "glue": "cadre-glue-v1"}).to_string().as_bytes())}),
        "getMempool" => json!({"size": 0, "txs": []}),
        "getDeposit" => json!({"processed": true}),
        // Solana
        "getSlot" => json!(513_000_000u64),
        "getMultipleAccounts" => {
            let prof = crate::solana::LOCALNET;
            let progs = [prof.reserve, prof.bridge, prof.storefront, prof.lightclient];
            let vals: Vec<Value> = p[0]
                .as_array()
                .unwrap()
                .iter()
                .map(|k| {
                    let k = k.as_str().unwrap_or("");
                    if progs.contains(&k) {
                        json!({"executable": true, "lamports": 1, "owner": "BPFLoaderUpgradeab1e11111111111111111111111", "data": ["", "base64"]})
                    } else if k == PAYER {
                        json!({"executable": false, "lamports": 2_500_000_000u64, "data": ["", "base64"]})
                    } else {
                        Value::Null
                    }
                })
                .collect();
            json!({"value": vals})
        }
        "getAccountInfo" => {
            let k = p[0].as_str().unwrap_or("");
            if k == crate::solana::b58(&crate::solana::bridge_config_pda()) {
                json!({"value": {"data": [b64(&bridge_config_bytes()), "base64"], "lamports": 1, "owner": crate::solana::LOCALNET.bridge}})
            } else if p[1]["encoding"] == "jsonParsed" {
                json!({"value": {"owner": crate::solana::TOKEN_2022_PROGRAM, "data": {"parsed": {"info": {"supply": "972262263353381", "decimals": 6}}}}})
            } else {
                json!({"value": null})
            }
        }
        "getTokenAccountBalance" => json!({"value": {"amount": "5000000"}}),
        "getBalance" => json!({"value": 1_000_000_000u64}),
        "getTransaction" => {
            let mut d = crate::solana::anchor_disc("buy_job_amm").to_vec();
            d.extend(35u64.to_le_bytes());
            d.extend(11_400_000u64.to_le_bytes());
            d.extend(0u64.to_le_bytes());
            json!({"slot": 513586907u64, "blockTime": now_ms() / 1000 - 600, "meta": {"err": null, "fee": 5000, "logMessages": [], "innerInstructions": [{"index": 1, "instructions": [
                        {"program": "system", "programId": "11111111111111111111111111111111", "parsed": {"type": "transfer", "info": {"lamports": 11_400_000u64, "source": PAYER, "destination": AGENT}}}]}]},
                   "transaction": {"message": {"accountKeys": [{"pubkey": PAYER, "signer": true}], "instructions": [
                        {"programId": crate::decode::COMPUTE_BUDGET, "data": "3"},
                        {"programId": crate::solana::LOCALNET.storefront, "data": bs58::encode(&d).into_string()}]}}})
        }
        _ => return Err(format!("mock has no {method}")),
    })
}

fn get_answer(k: &Knobs, path: &str) -> (u16, Value) {
    let path = path.split('?').next().unwrap_or(path);
    let ok = |v: Value| (200, v);
    match path {
        "/v1/ready" => (503, json!({"ready": false, "checks": {"node": {"ok": true}, "solana": {"ok": false, "detail": "https://api.devnet.solana.com: HTTP 429"}}, "upstreams": {"gateway": {"ok": true}}})),
        "/v1/summary" if k.thin_summary => ok(json!({"blocks": {"total": 100}, "light_client_lag": {"behind_final": 31}, "buyback": {}, "reserve": {}, "supply": {},
                                                     "status": {"solana_error": "https://api.devnet.solana.com: HTTP 429"}})),
        "/v1/summary" => ok(json!({
            "blocks": {"total": 100, "paid_job_blocks": 50, "paid_share": "50%"},
            "light_client_lag": {"tip_height": 99, "final_height": 69, "chain_height": 100, "behind_tip": 1, "behind_final": 31},
            "buyback": {"buys": 13, "spent_lamports": "29657293", "burned_spl": "1933526457817", "to_reserve_spl": "0", "vault_lamports": "3179402", "orders_waiting": 0, "last_buy_ts": now_ms() / 1000 - 1800},
            "reserve": {"tokens_spl": "812710098156204", "totals": {"funded_spl": "10000000000000"}, "last_crank_ts": now_ms() / 1000 - 7200},
            "supply": {"mint_supply_spl": "972262263353381", "initial_supply_spl": "1000000000000000", "burned_spl": "27737736646619", "bridge_vault_spl": "5000000", "agent_chain": {"escrow_balance": "999999995000000000"}},
            "storefront": {"orders": 22, "by_status": {"finalized": 14, "pending": 2}, "sol_settled": "266915654", "sol_pending": "20596708"},
            "withdrawals": {"count": 0, "released_on_solana": 0}})),
        "/v1/orders" => ok(json!({"orders": [{"nonce": "35", "job_id": "66".repeat(32)}]})),
        "/v1/jobs" => ok(json!({"jobs": [{"job_id": TICKET, "status": "completed", "kind": "ticket"}]})),
        "/v1/openai/models" => ok(json!({"data": [{"id": "llama-3.2-1b-instruct-w7a7"}]})),
        "/v1/openai/stats" => ok(json!({"requests_total": 78, "rounds_posted": 16, "decode_tokens_per_s_mean": 1.5})),
        "/v1/town" => ok(json!({"posts": [{"author": {"account": AGENT, "handle": "ferry_nan", "house": true}, "height": 90, "timestamp_ms": now_ms() - 600_000, "thought": {"status": "verified", "rows": 526}}], "pending": []})),
        "/v1/desktops" => ok(json!({"stopped": false, "capped": false, "cap_usd_per_day": 25, "spend_today": {"total_usd": 3.1}, "desktops_running": 2,
                                    "agents": [{"handle": "ferry_nan", "account": AGENT, "desktops": {"social": {"state": "running", "current": "reading the town"}}}]})),
        "/v1/desktops/ferry_nan" => ok(json!({"handle": "ferry_nan", "desktops": {"social": {"state": "running", "current": "reading the town"}}})),
        "/v1/hosted" => ok(json!({"runtime": "cadre-agents", "version": "0.1.0", "hosted_agents": 0})),
        "/v1/hosted/agents" => ok(json!({"total": 0, "agents": []})),
        "/v1/profiles" => ok(json!({"total": 1, "profiles": [{"account": AGENT, "handle": "ferry_nan", "name": "Old Nan", "is_agent": true, "house": true, "operator": PAYER, "operator_linked": true, "stats": {"posts": 1}}]})),
        p if p.starts_with("/v1/profiles/") && p.ends_with("/posts") => {
            ok(json!({"posts": [{"height": 90, "kind": "post", "body": "The tide is in.", "timestamp_ms": now_ms() - 600_000, "thought": {"status": "verified", "request_id": "req_ed20b1fb25b612d18d75a3b8"}}]}))
        }
        "/api/summary" => ok(json!({"index": {"complete": true, "blocks_indexed": 100, "blocks_on_chain": 100}})),
        p if p.starts_with("/api/agent/") => ok(json!({"account": AGENT, "name": null})),
        "/health" if k.relayer_down => (503, json!({"ok": false, "lastOkAt": "2020-01-01T00:00:00.000Z", "last": {"errors": ["token rpc http 429"]}, "cursors": {"deposit": "1", "withdrawal": "0"}})),
        "/health" => ok(json!({"ok": true, "wallet": PAYER, "network": "solana-devnet", "cursors": {"deposit": "3", "withdrawal": "0"}})),
        "/" | "/install-miner.sh" => (200, Value::Null),
        _ => (404, json!({"error": {"code": "not_found", "message": format!("no endpoint {path}")}})),
    }
}

/// Serve on a fresh loopback port; returns its URL.
pub fn serve(k: Knobs) -> String {
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}", l.local_addr().unwrap());
    let k = Arc::new(k);
    std::thread::spawn(move || {
        for conn in l.incoming().flatten() {
            let k = k.clone();
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
                    let mut body = vec![0; len];
                    if r.read_exact(&mut body).is_err() {
                        return;
                    }
                    let mut parts = first.split_whitespace();
                    let (method, path) = (parts.next().unwrap_or(""), parts.next().unwrap_or("/"));
                    let (code, out) = if method == "POST" {
                        let req: Value = serde_json::from_slice(&body).unwrap_or(Value::Null);
                        let resp = match rpc_answer(&k, req["method"].as_str().unwrap_or(""), &req["params"]) {
                            Ok(v) => json!({"jsonrpc": "2.0", "id": req["id"], "result": v}),
                            Err(m) => json!({"jsonrpc": "2.0", "id": req["id"], "error": {"code": -32601, "message": m}}),
                        };
                        (200, resp.to_string())
                    } else {
                        let (c, v) = get_answer(&k, path);
                        (c, if v.is_null() { "<html>cadre</html>".to_string() } else { v.to_string() })
                    };
                    let _ = write!(w, "HTTP/1.1 {code} X\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{out}", out.len());
                    let _ = w.flush();
                }
            });
        }
    });
    url
}

/// A context whose every endpoint is a mock: three nodes (the last one per `third`), Solana,
/// cadre-api, explorer, storefront, relayer and site on `main`; nothing box-local.
pub fn ctx(main: &str, third: &str, relayer: &str, text: bool) -> Ctx {
    let opts = Opts {
        network: crate::ctx::Network::Localnet,
        rpc: Some(main.into()),
        sol_rpc: Some(main.into()),
        key: None,
        sol_key: None,
        address: None,
        pool: None,
        credit_url: None,
        miner_bin: None,
        json: !text,
        wait: false,
        no_wait: false,
        endpoint: vec![],
        local: false,
        strict: false,
    };
    let mut c = Ctx::new(opts).unwrap();
    c.eps.nodes = vec![main.into(), main.into(), third.into()];
    c.eps.urls.clear();
    for (k, v) in [("api", format!("{main}/v1")), ("explorer", main.into()), ("storefront", main.into()), ("relayer", relayer.into()), ("site", main.into())] {
        c.eps.urls.insert(k.into(), v);
    }
    c.eps.relay_payers = vec![PAYER.into()];
    c.eps.mint = Some(AGENT.into());
    if text {
        c.ui = Ui::plain(Style::plain());
    }
    c
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::{exec, parse_line};
    use crate::health::Level;

    fn run(c: &Ctx, line: &str) -> crate::commands::Out {
        exec(c, &parse_line(line).unwrap(), false).unwrap_or_else(|e| panic!("{line}: {e:#}"))
    }

    fn state<'a>(j: &'a Value, name: &str) -> &'a str {
        j["checks"].as_array().unwrap().iter().find(|c| c["name"] == name).unwrap_or_else(|| panic!("no check {name}: {j}"))["state"].as_str().unwrap()
    }

    #[test]
    fn status_all_green_except_the_rate_limited_api() {
        crate::solana::set_network(crate::ctx::Network::Localnet);
        let u = serve(Knobs::default());
        let c = ctx(&u, &u, &u, false);
        let o = run(&c, "status");
        let j = &o.json;
        assert_eq!(o.code, 0, "{j}");
        for name in ["agreement", "tip", "solana rpc", "program bridge", "program lightclient", "CADRE mint", "light client", "explorer", "storefront-x402", "relayer", "gateway", "site", "settle", "verifiable AI", "house swarm", "house desktops", "user-agent runtime", "buyback", "supply", "mining", "bridge"] {
            assert_eq!(state(j, name), "pass", "{name}: {j}");
        }
        assert_eq!(state(j, "cadre-api"), "warn");
        assert_eq!(state(j, "docs site"), "skip");
        assert_eq!(state(j, "relay payer XQUUCs"), "pass");
        assert_eq!(j["result"], "warn");
        let detail = |n: &str| j["checks"].as_array().unwrap().iter().find(|c| c["name"] == n).unwrap()["detail"].as_str().unwrap().to_string();
        assert!(detail("bridge").starts_with("OK: vault 5 CADRE = outside escrow 5 CADRE"), "{}", detail("bridge"));
        assert!(detail("mining").contains("paid-job blocks 30 of 60"), "{}", detail("mining"));
        assert!(detail("verifiable AI").contains("cadre-glue-v1"));
        assert!(detail("cadre-api").contains("HTTP 429"));
        // --strict turns the warning into exit 1
        let mut c2 = ctx(&u, &u, &u, false);
        c2.opts.strict = true;
        assert_eq!(run(&c2, "status").code, 1);
    }

    #[test]
    fn status_fails_on_a_fork_and_a_dead_relayer() {
        crate::solana::set_network(crate::ctx::Network::Localnet);
        let good = serve(Knobs::default());
        let forked = serve(Knobs { fork: true, ..Default::default() });
        let bad_relayer = serve(Knobs { relayer_down: true, ..Default::default() });
        let c = ctx(&good, &forked, &bad_relayer, false);
        let o = run(&c, "status");
        assert_eq!(o.code, 1);
        assert_eq!(state(&o.json, "agreement"), "fail");
        assert_eq!(state(&o.json, "relayer"), "fail");
        assert_eq!(o.json["result"], "fail");
        // a node that does not answer
        let c = ctx(&good, "http://127.0.0.1:9", &good, false);
        let o = run(&c, "status");
        assert_eq!(o.code, 1);
        assert_eq!(state(&o.json, "node 127.0.0.1:9"), "fail");
        // health lines: OK / WARN / FAIL words, no SKIP lines
        let o = run(&c, "status --health");
        assert!(o.raw && o.text.lines().all(|l| l.starts_with("OK cli:") || l.starts_with("WARN cli:") || l.starts_with("FAIL cli:")), "{}", o.text);
        assert!(o.text.contains("FAIL cli:node_127.0.0.1:9"));
    }

    #[test]
    fn status_text_screen() {
        crate::solana::set_network(crate::ctx::Network::Localnet);
        let u = serve(Knobs::default());
        let c = ctx(&u, &u, &u, true);
        let o = run(&c, "status");
        let s = o.view.unwrap().render(&Style::plain());
        for want in ["[ Chain ]", "[ Solana ]", "[ Services ]", "[ Agents ]", "[ Economy ]", "agreement", "PASS", "WARN", "SKIP", "result WARN"] {
            assert!(s.contains(want), "missing {want}:\n{s}");
        }
        assert!(!s.contains('\u{2014}'));
        // every group's box has the same width
        let widths: std::collections::BTreeSet<usize> = s.lines().filter(|l| l.starts_with('╭')).map(|l| l.chars().count()).collect();
        assert!(widths.len() <= 2, "{widths:?}"); // the Box group may be the only other width
    }

    #[test]
    fn ping_targets_and_latency() {
        crate::solana::set_network(crate::ctx::Network::Localnet);
        let u = serve(Knobs::default());
        let c = ctx(&u, &u, &u, false);
        let o = run(&c, "ping node0 -c 3");
        assert_eq!(o.code, 0);
        assert_eq!(o.json["latency"][0]["n"], 3);
        let o = run(&c, "ping explorer");
        assert_eq!(o.json["checks"][0]["state"], "pass");
        let o = run(&c, &format!("ping {u}/api/summary"));
        assert_eq!(o.json["checks"][0]["state"], "pass");
        assert!(exec(&c, &parse_line("ping nosuch").unwrap(), false).is_err());
        let o = run(&c, "ping --list");
        assert!(o.json["urls"]["api"].as_str().unwrap().ends_with("/v1"));
        let bad = serve(Knobs { relayer_down: true, ..Default::default() });
        let c = ctx(&u, &u, &bad, false);
        assert_eq!(run(&c, "ping relayer").code, 1);
    }

    #[test]
    fn tx_both_chains_in_words() {
        crate::solana::set_network(crate::ctx::Network::Localnet);
        let u = serve(Knobs::default());
        let c = ctx(&u, &u, &u, false);
        let h = hex::encode(post_tx().hash());
        let o = run(&c, &format!("tx {h}"));
        assert_eq!(o.json["chain"], "cadre");
        assert_eq!(o.json["state"], "included");
        let words = o.json["decoded"]["words"].as_str().unwrap();
        assert!(words.starts_with("@ferry_nan posted: \"The tide is in.\""), "{words}");
        assert_eq!(o.json["kind"], "Post"); // getTx fields are kept
        let o = run(&c, &format!("tx {SOL_SIG}"));
        assert_eq!(o.json["chain"], "solana");
        assert_eq!(o.json["success"], true);
        assert_eq!(o.json["instructions"][1], "storefront buy_job_amm: bought AI job order #35 for 0.0114 SOL");
        assert!(o.json["inner_words"][0].as_str().unwrap().starts_with("SOL transfer"));
        let c = ctx(&u, &u, &u, true);
        let s = run(&c, &format!("tx {h}")).view.unwrap().render(&Style::plain());
        assert!(s.contains("[ Transaction: Post ]") && s.contains("Thought receipt") && s.contains("INCLUDED"), "{s}");
    }

    #[test]
    fn account_block_agent_agents() {
        crate::solana::set_network(crate::ctx::Network::Localnet);
        let u = serve(Knobs::default());
        let c = ctx(&u, &u, &u, false);
        let o = run(&c, "account @ferry_nan");
        assert_eq!(o.json["account"], AGENT);
        assert_eq!(o.json["solana"]["sol_lamports"], "1000000000");
        assert_eq!(o.json["solana"]["cadre_spl"], "5000000");
        let o = run(&c, "block 90");
        assert_eq!(o.json["decoded"][0]["kind"], "Post");
        let o = run(&c, "agent @ferry_nan");
        assert_eq!(o.json["account"], AGENT);
        assert_eq!(o.json["town"]["handle"], "ferry_nan");
        assert_eq!(o.json["posts"][0]["thought"]["status"], "verified");
        assert_eq!(o.json["desktops"]["handle"], "ferry_nan");
        let o = run(&c, "agents");
        assert_eq!(o.json["agents"][0]["health"], "ok");
        assert_eq!(o.json["agents"][0]["kind"], "house");
        let c = ctx(&u, &u, &u, true);
        let s = run(&c, "agent ferry_nan").view.unwrap().render(&Style::plain());
        assert!(s.contains("[ Town profile ]") && s.contains("linked both ways") && s.contains("[ Recent posts ]"), "{s}");
    }

    #[test]
    fn ticket_verdict_and_watch() {
        crate::solana::set_network(crate::ctx::Network::Localnet);
        let u = serve(Knobs::default());
        let c = ctx(&u, &u, &u, false);
        let o = run(&c, &format!("verify {TICKET}"));
        assert_eq!(o.json["verdict"], "accepted");
        assert_eq!(o.json["replayable"], true);
        assert_eq!(o.code, 0);
        let o = run(&c, "watch --from 99 --count 2");
        assert_eq!(o.json["blocks"], 2);
        assert_eq!(o.json["txs"], 2);
    }

    #[test]
    fn missing_figures_are_warnings_not_zeros() {
        crate::solana::set_network(crate::ctx::Network::Localnet);
        let u = serve(Knobs { thin_summary: true, ..Default::default() });
        let c = ctx(&u, &u, &u, false);
        let o = run(&c, "status");
        for name in ["buyback", "reserve", "supply"] {
            assert_eq!(state(&o.json, name), "warn", "{name}");
            let d = o.json["checks"].as_array().unwrap().iter().find(|x| x["name"] == name).unwrap()["detail"].as_str().unwrap().to_string();
            assert!(d.contains("HTTP 429") && !d.contains("0 CADRE"), "{d}");
        }
    }

    #[test]
    fn levels_order() {
        assert!(Level::Fail > Level::Warn && Level::Warn > Level::Skip && Level::Skip > Level::Pass);
    }
}
