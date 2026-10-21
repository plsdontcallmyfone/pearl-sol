//! A small RPC client: CPU fallback-block miner and a transaction load generator for
//! the devnet. Job blocks (matrices from a job's data_uri) are services/miner's job.

use std::time::{Duration, Instant};

use agent_types::tx::{deposit_message, pubkey_from_seed, sign_plain};
use agent_types::{AttesterSig, Header, Pubkey, Tx, TxBody};
use anyhow::{anyhow, bail, Context, Result};
use base64::Engine as _;
use serde_json::{json, Value};

pub struct Client {
    pub url: String,
    agent: ureq::Agent,
}

impl Client {
    pub fn new(url: &str) -> Self {
        Self { url: url.to_string(), agent: ureq::AgentBuilder::new().timeout(Duration::from_secs(60)).build() }
    }

    pub fn call(&self, method: &str, params: Value) -> Result<Value> {
        let v: Value = self
            .agent
            .post(&self.url)
            .send_json(json!({"jsonrpc": "2.0", "id": 1, "method": method, "params": params}))
            .map_err(|e| anyhow!("{method}: {e}"))?
            .into_json()?;
        if let Some(e) = v.get("error").filter(|e| !e.is_null()) {
            bail!("{method}: {}", e.get("message").and_then(|m| m.as_str()).unwrap_or("error"));
        }
        Ok(v.get("result").cloned().unwrap_or(Value::Null))
    }
}

fn b64(b: &[u8]) -> String {
    base64::engine::general_purpose::STANDARD.encode(b)
}

/// Mine fallback blocks: `count` blocks (0 = forever). Returns (blocks, seconds).
pub fn mine(rpc: &str, miner: Pubkey, count: u64, job_mode: &str) -> Result<(u64, f64)> {
    let c = Client::new(rpc);
    let start = Instant::now();
    let mut found = 0u64;
    let mut nonce_base = 0u64;
    while count == 0 || found < count {
        let job_param = if job_mode == "auto" { json!(null) } else { json!("fallback") };
        let mut p = json!({"miner": bs58::encode(miner).into_string()});
        if !job_param.is_null() {
            p["job_id"] = job_param;
        }
        let t = c.call("getBlockTemplate", p)?;
        if !t["job"].is_null() {
            bail!("template is a job block; this miner only mines fallback blocks (use --job-mode fallback)");
        }
        let hdr = Header::decode(&hex::decode(t["header"].as_str().context("header")?)?)?;
        let ph = pearl_job::BlockHeader::from_bytes(&hdr.encode())?;
        let f = &t["fallback"];
        let params = pearl_job::FallbackParams {
            m: f["m"].as_u64().context("m")? as u32,
            n: f["n"].as_u64().context("n")? as u32,
            config: pearl_job::MiningConfig::from_bytes(&hex::decode(f["config"].as_str().context("config")?)?)?,
        };
        let range = nonce_base..nonce_base + 64;
        nonce_base += 64;
        // weak blocks (docs/SPEED.md 2.3): mine against the weak bound, send weak hits, keep going
        let weak = t["weak_target_bits"].as_u64().map(|b| b as u32);
        let mut strong_hit = None;
        for n in range {
            let mined = match weak {
                Some(wb) => pearl_job::pow::mine_header_nonce_at(&ph, &pearl_job::BlockWork::Fallback(params), n, wb)?,
                None => pearl_job::pow::mine_header_nonce(&ph, &pearl_job::BlockWork::Fallback(params), n)?.map(|b| (b, true)),
            };
            match mined {
                Some((b, true)) => {
                    strong_hit = Some(b);
                    break;
                }
                Some((b, false)) => {
                    let r = c.call("submitWeakBlock", json!({"header": hex::encode(b.header.to_bytes()), "proof": b64(&b.proof), "template_id": t["template_id"]}));
                    tracing::debug!(height = hdr.height, ?r, "weak block");
                }
                None => {}
            }
        }
        let Some(mb) = strong_hit else { continue };
        let r = c.call(
            "submitBlock",
            json!({"header": hex::encode(mb.header.to_bytes()), "proof": b64(&mb.proof), "template_id": t["template_id"]}),
        )?;
        if r["accepted"].as_bool() == Some(true) {
            found += 1;
            tracing::info!(height = hdr.height, txs = t["tx_count"].as_u64().unwrap_or(0), "mined block");
        } else {
            tracing::warn!("block rejected: {}", r["reason"]);
        }
    }
    Ok((found, start.elapsed().as_secs_f64()))
}

/// Fund `senders` accounts through attested BridgeUnlocks, then send `per_sender`
/// transfers from each. Returns the tx hashes sent.
pub fn load(rpc: &str, attester_seed: [u8; 32], senders: u8, per_sender: u64, base_nonce: u64) -> Result<Vec<String>> {
    let c = Client::new(rpc);
    let info = c.call("getChainInfo", json!({}))?;
    let chain_id = info["chain_id"].as_u64().context("chain_id")? as u32;
    let params = c.call("getParams", json!({}))?;
    let min_fee: u64 = params["min_fee"].as_str().unwrap_or("1000").parse()?;
    let relayer = blake3::hash(b"pearl-sol devnet relayer").as_bytes().to_owned();
    let rpk = pubkey_from_seed(&relayer);
    let mut rnonce = c.call("getAccount", json!({"account": bs58::encode(rpk).into_string()}))?["nonce"].as_u64().unwrap_or(0);
    let seeds: Vec<[u8; 32]> = (0..senders).map(|i| *blake3::hash(&[b"pearl-sol devnet sender ".as_slice(), &[i]].concat()).as_bytes()).collect();
    let mut hashes = Vec::new();
    for (i, s) in seeds.iter().enumerate() {
        let to = pubkey_from_seed(s);
        let acc = c.call("getAccount", json!({"account": bs58::encode(to).into_string()}))?;
        let bal: u64 = acc["balance"].as_str().unwrap_or("0").parse()?;
        if bal < (per_sender + 1) * (min_fee + 1) {
            let amount_spl = 1_000_000_000u64; // 1000 tokens
            let sn = base_nonce + i as u64;
            let msg = deposit_message(sn, &to, amount_spl);
            let sig = AttesterSig { pubkey: pubkey_from_seed(&attester_seed), sig: sign_plain(&attester_seed, &msg) };
            let tx = Tx::signed(&relayer, chain_id, rnonce, 0, TxBody::BridgeUnlock { solana_nonce: sn, recipient: to, amount_spl, job_order: None, sigs: vec![sig] });
            rnonce += 1;
            c.call("sendTx", json!({"tx": b64(&tx.encode())}))?;
        }
    }
    // wait for funding
    for s in &seeds {
        let to = bs58::encode(pubkey_from_seed(s)).into_string();
        let t0 = Instant::now();
        loop {
            let acc = c.call("getAccount", json!({"account": to}))?;
            if acc["balance"].as_str().unwrap_or("0") != "0" {
                break;
            }
            if t0.elapsed() > Duration::from_secs(300) {
                bail!("funding not mined after 300 s (is a miner running?)");
            }
            std::thread::sleep(Duration::from_millis(200));
        }
    }
    for s in &seeds {
        let from = pubkey_from_seed(s);
        let n0 = c.call("getAccount", json!({"account": bs58::encode(from).into_string()}))?["nonce"].as_u64().unwrap_or(0);
        for k in 0..per_sender {
            let tx = Tx::signed(s, chain_id, n0 + k, min_fee, TxBody::Transfer { to: [7; 32], amount: 1 });
            let r = c.call("sendTx", json!({"tx": b64(&tx.encode())}))?;
            hashes.push(r["tx_hash"].as_str().unwrap_or_default().to_string());
        }
    }
    Ok(hashes)
}
