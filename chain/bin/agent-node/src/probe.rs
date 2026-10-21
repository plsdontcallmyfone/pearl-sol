//! `agent-node preconf-probe`: measure the time from sending a tx to seeing it pre-confirmed
//! (docs/SPEED.md 2.4) on a different node, then included. One JSON line per tx on stdout.

use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use agent_types::tx::{deposit_message, pubkey_from_seed, sign_plain};
use agent_types::{AttesterSig, Tx, TxBody};
use anyhow::{bail, Context, Result};
use base64::Engine as _;
use serde_json::{json, Value};

use crate::miner::Client;

fn now_ms() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
}

pub struct ProbeArgs {
    pub rpcs: Vec<String>,
    /// Attester seed: fund the sender with a BridgeUnlock first (local test chains).
    pub attester_seed: Option<[u8; 32]>,
    /// The sender's seed (funded already, or funded through `attester_seed`).
    pub sender_seed: [u8; 32],
    pub count: u64,
    pub spacing: Duration,
    pub timeout: Duration,
}

pub fn run(a: ProbeArgs) -> Result<()> {
    if a.rpcs.len() < 2 {
        bail!("give at least two --rpc endpoints (the tx goes to one, the status is read on another)");
    }
    let cs: Vec<Client> = a.rpcs.iter().map(|u| Client::new(u)).collect();
    let info = cs[0].call("getChainInfo", json!({}))?;
    let chain_id = info["chain_id"].as_u64().context("chain_id")? as u32;
    let params = cs[0].call("getParams", json!({}))?;
    let min_fee: u64 = params["min_fee"].as_str().unwrap_or("1000").parse()?;
    let from = pubkey_from_seed(&a.sender_seed);
    let b58 = |k: &[u8; 32]| bs58::encode(k).into_string();
    if let Some(att) = a.attester_seed {
        let relayer = *blake3::hash(b"pearl-sol preconf probe relayer").as_bytes();
        let rpk = pubkey_from_seed(&relayer);
        let rnonce = cs[0].call("getAccount", json!({"account": b58(&rpk)}))?["nonce"].as_u64().unwrap_or(0);
        let sn = 2_000_000 + now_ms() % 1_000_000;
        let amount_spl = 1_000_000u64;
        let msg = deposit_message(sn, &from, amount_spl);
        let sig = AttesterSig { pubkey: pubkey_from_seed(&att), sig: sign_plain(&att, &msg) };
        let tx = Tx::signed(&relayer, chain_id, rnonce, 0, TxBody::BridgeUnlock { solana_nonce: sn, recipient: from, amount_spl, job_order: None, sigs: vec![sig] });
        cs[0].call("sendTx", json!({"tx": base64::engine::general_purpose::STANDARD.encode(tx.encode())}))?;
        let t0 = Instant::now();
        loop {
            let acc = cs[0].call("getAccount", json!({"account": b58(&from)}))?;
            if acc["balance"].as_str().unwrap_or("0") != "0" {
                break;
            }
            if t0.elapsed() > a.timeout {
                bail!("funding not mined in time");
            }
            std::thread::sleep(Duration::from_millis(500));
        }
    }
    let mut nonce = cs[0].call("getAccount", json!({"account": b58(&from)}))?["nonce"].as_u64().unwrap_or(0);
    struct Rec {
        h: String,
        sent: u64,
        send_to: usize,
        watch: usize,
        pre: Option<u64>,
        pre_v: Value,
        inc: Option<u64>,
        height: Option<u64>,
    }
    let mut recs: Vec<Rec> = Vec::new();
    let poll = |cs: &[Client], recs: &mut Vec<Rec>| -> Result<()> {
        for r in recs.iter_mut().filter(|r| r.inc.is_none()) {
            let v = cs[r.watch].call("getTxStatus", json!({"tx_hash": r.h}))?;
            let st = v["status"].as_str().unwrap_or("");
            if st == "preconfirmed" && r.pre.is_none() {
                r.pre = Some(now_ms());
                r.pre_v = v["preconf"].clone();
            }
            if st == "included" || st == "final" {
                r.inc = Some(now_ms());
                r.height = v["height"].as_u64();
            }
        }
        Ok(())
    };
    for i in 0..a.count {
        let send_to = (i as usize) % cs.len();
        let watch = (send_to + 1) % cs.len();
        let tx = Tx::signed(&a.sender_seed, chain_id, nonce, min_fee, TxBody::Transfer { to: [7; 32], amount: 1 });
        nonce += 1;
        let h = hex::encode(tx.hash());
        let sent = now_ms();
        cs[send_to].call("sendTx", json!({"tx": base64::engine::general_purpose::STANDARD.encode(tx.encode())}))?;
        recs.push(Rec { h, sent, send_to, watch, pre: None, pre_v: Value::Null, inc: None, height: None });
        let t0 = Instant::now();
        while t0.elapsed() < a.spacing {
            poll(&cs, &mut recs)?;
            std::thread::sleep(Duration::from_millis(50));
        }
    }
    let t0 = Instant::now();
    while recs.iter().any(|r| r.inc.is_none()) && t0.elapsed() < a.timeout {
        poll(&cs, &mut recs)?;
        std::thread::sleep(Duration::from_millis(50));
    }
    for (i, r) in recs.iter().enumerate() {
        let block_ts = match r.height {
            Some(hh) => cs[r.watch].call("getBlock", json!({"height": hh}))?["header_fields"]["timestamp_ms"].as_u64(),
            None => None,
        };
        println!(
            "{}",
            json!({"i": i, "tx": r.h, "sent_ms": r.sent, "sent_to": r.send_to, "watched": r.watch,
                   "preconf_ms": r.pre.map(|p| p - r.sent), "included_ms": r.inc.map(|p| p - r.sent), "height": r.height,
                   "block_timestamp_ms": block_ts, "preconf": r.pre_v})
        );
    }
    Ok(())
}
