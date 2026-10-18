//! Tests only: a canned JSON-RPC node on a loopback ephemeral port, so every command's
//! text-mode output path (steps + box) and its JSON run end to end without a stack.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use std::path::PathBuf;

use base64::Engine as _;
use serde_json::{json, Value};

use crate::commands::{exec, Cmd};
use crate::ctx::{Ctx, Opts};
use crate::term::{Style, Ui};

pub const OTHER: &str = "7xKXtg2CW87d97TXJSDpbD5jBkheTqA83TZRuJosgAsU";

fn job() -> Value {
    json!({"job_id": "ab".repeat(32), "status": "completed", "poster": OTHER, "worker_pay": "5000000000", "protocol_fee": "1000000000",
           "m": 64, "n": 64, "k": 2048, "blocks_mined": 1, "max_blocks": 10, "deadline_height": 900, "posted_height": 4, "closed_height": 9,
           "evaluators": [OTHER], "threshold": 1, "data_uri": "blob:00", "deliverer": OTHER, "result_hash": "cd".repeat(32), "result_uri": "blob:01",
           "cap_remaining": "0", "subsidy_paid": "800000000"})
}

/// A storefront Config with the buyback fields (1 attester), as the program writes it.
pub fn store_config_bytes() -> Vec<u8> {
    let mint = crate::solana::pk(OTHER);
    let mut d = vec![0u8; 8];
    d.extend([1u8; 32]); // admin
    d.extend(mint); // mint
    d.extend([2u8; 32]); // bridge
    d.push(0); // paused
    d.extend(2000u16.to_le_bytes());
    d.extend(300u16.to_le_bytes());
    d.extend(10_000_000u64.to_le_bytes());
    d.extend(10_000_000_000u64.to_le_bytes());
    d.extend(8192u32.to_le_bytes());
    d.extend(10_000u32.to_le_bytes());
    d.extend(100_000u64.to_le_bytes());
    d.push(1); // threshold
    d.extend(1u32.to_le_bytes());
    d.extend([3u8; 32]); // attester
    d.extend(4u64.to_le_bytes()); // orders
    d.extend(2_000_000_000u64.to_le_bytes()); // total_sol
    d.extend(7u64.to_le_bytes()); // total_tokens
    d.extend([254, 253, 2]); // bump, trader_bump, finalize_modes
    d.extend([4u8; 32]); // light_client
    d.extend(1000u16.to_le_bytes()); // buyback_bps
    d.push(0); // dest reserve
    d.extend(crate::solana::pk("4FkQcoCMJdRpZW7bK3wnaMoLjYdWaJBDB8xTUGskLY6p"));
    d.extend([0u8; 32]); // cranker: anyone
    for v in [1_000_000_000u64, 1_000_000, 600, 1_760_000_000, 300_000_000, 250_000_000, 9_000_000_000, 9_000_000_000, 0, 1, 1, 12, 12] {
        d.extend(v.to_le_bytes());
    }
    d.extend(20_000_000u64.to_le_bytes()); // bb_locked
    d.extend(5_000_000u64.to_le_bytes()); // bb_refunded
    d.resize(d.len() + 200, 0);
    d
}

/// One BuybackExecuted event payload (disc + Borsh), as the program logs it.
pub fn buyback_event_bytes() -> Vec<u8> {
    let mut d = crate::solana::event_disc("BuybackExecuted").to_vec();
    d.extend(1u64.to_le_bytes()); // buy
    d.push(0); // venue curve
    d.extend(250_000_000u64.to_le_bytes()); // sol_in
    d.extend(9_000_000_000u64.to_le_bytes()); // tokens_out
    d.extend(9_000_000_000u64.to_le_bytes()); // routed
    d.push(0); // destination reserve
    d.extend([5u8; 32]);
    d.extend(9_100_000_000u64.to_le_bytes()); // quote
    for v in [3u64, 9, 3, 300_000_000, 50_000_000, 250_000_000, 9_000_000_000, 9_000_000_000, 0] {
        d.extend(v.to_le_bytes());
    }
    d
}

fn answer(method: &str, params: &Value) -> Result<Value, String> {
    let b64 = |b: Vec<u8>| base64::engine::general_purpose::STANDARD.encode(b);
    Ok(match method {
        "getAccountInfo" if params[0].as_str() == Some(&crate::solana::b58(&crate::solana::storefront_config_pda())) => {
            json!({"value": {"data": [b64(store_config_bytes()), "base64"], "lamports": 1, "owner": crate::solana::STOREFRONT_PROGRAM}})
        }
        "getAccountInfo" if params[0].as_str() == Some(OTHER) => {
            let mut m = vec![0u8; 82];
            m[36..44].copy_from_slice(&999_000_000_000_000u64.to_le_bytes());
            json!({"value": {"data": [b64(m), "base64"], "lamports": 1, "owner": crate::solana::TOKEN_2022_PROGRAM}})
        }
        "getBalance" => json!({"value": 890_880u64 + 50_000_000 + 20_000_000}),
        "getSignaturesForAddress" => json!([{"signature": "5sig", "slot": 77, "err": null, "blockTime": 1_760_000_000}]),
        "getTransaction" => json!({"meta": {"logMessages": ["Program DxuuzbuTX32Na4yVDpjo7ZBZQWufFGjzGZASMRpPgXkr invoke [1]", format!("Program data: {}", b64(buyback_event_bytes()))]}}),
        "getChainInfo" => json!({"height": 20, "finalized_height": 14, "finality_k": 6, "chain_id": 7, "peers": 1, "syncing": false, "target_bits": 1}),
        "getParams" => json!({"min_fee": "1000", "attesters": [OTHER], "emission_ppm": 1000, "min_protocol_fee_effective": "1"}),
        "getEscrow" => json!({"escrow_balance": "999000000000000000", "pool_balance": "100", "total_supply": "1000000000000000000", "outside_escrow": "1000000000000000"}),
        "getAccount" => json!({"balance": "5000000000", "nonce": 2, "route": null}),
        "getAgent" => json!({"name": "demo-agent", "uri": "https://x", "registered_height": 3, "updated_height": 3}),
        "getRoute" => Value::Null,
        "getAccountTxs" => json!({"txs": [{"tx_hash": "ef".repeat(32), "kind": "Transfer", "role": "sender", "height": 10, "fee": "1000", "final": true}]}),
        "listJobs" => json!({"total": 1, "jobs": [job()]}),
        "getJob" => job(),
        "getBlock" => {
            let h = params["height"].as_u64().unwrap_or(0);
            json!({"hash": "aa".repeat(32), "header_fields": {"miner": OTHER, "job_id": "00".repeat(32), "height": h}, "reward": "1000", "subsidy": "0", "final": h <= 14, "txs": []})
        }
        "getBridgeEvents" => json!({"withdrawal_count": 0, "deposit_count": 0, "withdrawals": []}),
        "getMempool" => json!({"txs": []}),
        "sendTx" => {
            let b = base64::engine::general_purpose::STANDARD.decode(params["tx"].as_str().unwrap_or("")).map_err(|e| e.to_string())?;
            let tx = agent_types::tx::Tx::decode(&b).map_err(|e| format!("{e:?}"))?;
            json!({"tx_hash": hex::encode(tx.hash())})
        }
        "getTx" => json!({"tx_hash": params["tx_hash"], "status": "included", "height": 10, "kind": "Transfer", "from": OTHER, "nonce": 2, "fee": "1000", "confirmations": 11}),
        _ => return Err(format!("stub has no {method}")),
    })
}

/// Serve canned answers forever on a fresh loopback port; returns its URL.
pub fn serve() -> String {
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}", l.local_addr().unwrap());
    std::thread::spawn(move || {
        for conn in l.incoming().flatten() {
            std::thread::spawn(move || {
                let mut r = BufReader::new(conn.try_clone().unwrap());
                let mut w = conn;
                loop {
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
                    let req: Value = serde_json::from_slice(&body).unwrap_or(Value::Null);
                    let resp = match answer(req["method"].as_str().unwrap_or(""), &req["params"]) {
                        Ok(v) => json!({"jsonrpc": "2.0", "id": req["id"], "result": v}),
                        Err(m) => json!({"jsonrpc": "2.0", "id": req["id"], "error": {"code": -32601, "message": m}}),
                    };
                    let out = resp.to_string();
                    let _ = write!(w, "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{out}", out.len());
                    let _ = w.flush();
                }
            });
        }
    });
    url
}

fn key_file() -> PathBuf {
    let p = std::env::temp_dir().join(format!("pearl-cli-test-key-{}.json", std::process::id()));
    let sk = ed25519_dalek::SigningKey::from_bytes(&[9; 32]);
    let mut b = sk.to_bytes().to_vec();
    b.extend_from_slice(&sk.verifying_key().to_bytes());
    std::fs::write(&p, serde_json::to_string(&b).unwrap()).unwrap();
    p
}

pub fn ctx(url: &str, text_mode: bool) -> Ctx {
    let opts = Opts {
        rpc: url.into(),
        sol_rpc: url.into(),
        key: Some(key_file()),
        sol_key: None,
        address: None,
        pool: "http://127.0.0.1:9".into(),
        credit_url: None,
        miner_bin: None,
        json: !text_mode,
        wait: false,
        no_wait: false,
    };
    let mut c = Ctx::new(opts).unwrap();
    if text_mode {
        c.ui = Ui::plain(Style::plain());
    }
    c
}

pub fn run(c: &Ctx, line: &str) -> crate::commands::Out {
    let cmd: Cmd = crate::commands::parse_line(line).unwrap();
    exec(c, &cmd, false).unwrap_or_else(|e| panic!("{line}: {e:#}"))
}
