//! Devnet helper for the live data-availability check: funds a throwaway poster (and the
//! miner payout, for delivery fees) with attested BridgeUnlocks, generates a 64 x 64 x 1024
//! job, puts its PSD1 data blob on node A (`putBlob`) and posts the job there with
//! `data_uri = blob:<id>`. Mine it with a miner pointed at another node. Devnet only (the
//! attester key is the localnet one).
//!
//!   cargo run --release -p pearl-miner --example da_post_job -- <rpc A> <attester-key> <miner-payout-key> <solana_nonce base>

use std::time::Duration;

use anyhow::{Context, Result};
use ed25519_dalek::{Signer, SigningKey};
use serde_json::json;

use pearl_job::{data, job_key, JobHeader, MiningConfig};
use pearl_node::tx::{self, kind, Body, Tx};
use pearl_node::{b58, read_key_file, HttpNode, NodeApi};

fn unlock(node: &HttpNode, chain_id: u32, attester: &SigningKey, relayer: &SigningKey, sol_nonce: u64, to: &[u8; 32], amount_spl: u64) -> Result<()> {
    let dep = node.call("getDeposit", json!({"solana_nonce": sol_nonce}))?;
    if dep["processed"].as_bool() == Some(true) {
        return Ok(());
    }
    let mut msg = b"pearl-sol:deposit:v1".to_vec();
    msg.extend_from_slice(&sol_nonce.to_le_bytes());
    msg.extend_from_slice(to);
    msg.extend_from_slice(&amount_spl.to_le_bytes());
    let mut body = msg[20..].to_vec();
    body.push(0);
    body.push(1);
    body.extend_from_slice(&attester.verifying_key().to_bytes());
    body.extend_from_slice(&attester.sign(&msg).to_bytes());
    let me = relayer.verifying_key().to_bytes();
    let n = node.get_account(&me)?.nonce;
    let w = tx::sign(&Tx { chain_id, from: me, nonce: n, fee: 0, body: Body::Raw { kind: kind::BRIDGE_UNLOCK, body } }, relayer)?;
    println!("{}", json!({"event": "bridge_unlock", "to": b58(to), "tx": node.send_tx(&w)?, "amount_spl": amount_spl}));
    Ok(())
}

fn main() -> Result<()> {
    let a: Vec<String> = std::env::args().skip(1).collect();
    anyhow::ensure!(a.len() == 4, "usage: da_post_job <rpc A> <attester-key> <miner-payout-key> <solana_nonce base>");
    let node = HttpNode::new(&a[0]);
    let attester = read_key_file(std::path::Path::new(&a[1]))?;
    let miner = read_key_file(std::path::Path::new(&a[2]))?.verifying_key().to_bytes();
    let base: u64 = a[3].parse()?;
    let info = node.call("getChainInfo", json!({}))?;
    let chain_id = info["chain_id"].as_u64().context("chain_id")? as u32;
    let params = node.call("getParams", json!({}))?;
    let min_fee: u64 = params["min_fee"].as_str().context("min_fee")?.parse()?;
    let min_p: u64 = params["min_protocol_fee_effective"].as_str().context("min_protocol_fee_effective")?.parse()?;
    let poster = SigningKey::from_bytes(blake3::hash(format!("pearl-sol da devnet poster {base}").as_bytes()).as_bytes());
    let me = poster.verifying_key().to_bytes();
    unlock(&node, chain_id, &attester, &poster, base, &me, 20_000_000)?;
    unlock(&node, chain_id, &attester, &poster, base + 1, &miner, 1_000_000)?;
    while node.get_account(&me)?.balance == 0 || node.get_account(&miner)?.balance == 0 {
        std::thread::sleep(Duration::from_millis(500));
    }

    // job terms (docs/RPC.md "Job ids and Pearl job header")
    let (m, n, k) = (64u32, 64u32, 2048u32);
    let (w, p, max_blocks) = (5_000_000_000u64, 2_000_000_000u64.max(min_p), 3u32);
    let height = info["height"].as_u64().unwrap_or(0);
    let deadline = height + 2000;
    let nonce = node.get_account(&me)?.nonce;
    let job_id: [u8; 32] = *blake3::hash(&[&me[..], &nonce.to_le_bytes()].concat()).as_bytes();
    let mut t = b"pearl-sol:terms:v2".to_vec();
    t.extend_from_slice(&me);
    t.extend_from_slice(&job_id);
    for x in [m, n, k] {
        t.extend_from_slice(&x.to_le_bytes());
    }
    t.extend_from_slice(&w.to_le_bytes());
    t.extend_from_slice(&p.to_le_bytes());
    t.extend_from_slice(&deadline.to_le_bytes());
    t.extend_from_slice(&max_blocks.to_le_bytes());
    let terms = *blake3::hash(&t).as_bytes();
    let cfg = MiningConfig::localnet_default();
    let jh = JobHeader { job_pubkey: job_id, terms_hash: terms, created_slot: 0, target_bits: 0 };
    let jk = job_key(&jh, &cfg);
    let (am, bm) = data::random_matrices(m as usize, n as usize, k as usize, Some(base));
    let (ha, hb) = pearl_job::commit(&jk, &am, &bm, k as usize, n as usize);
    let blob = data::encode_job_blob(m, n, k, &am, &bm)?;
    let uri = node.put_blob(&blob)?;
    println!("{}", json!({"event": "blob_put", "uri": uri, "size": blob.len(), "rpc": a[0]}));

    let mut body = Vec::new();
    for x in [m, n, k] {
        body.extend_from_slice(&x.to_le_bytes());
    }
    body.extend_from_slice(&cfg.to_bytes());
    body.extend_from_slice(&ha);
    body.extend_from_slice(&hb);
    body.extend_from_slice(&(uri.len() as u16).to_le_bytes());
    body.extend_from_slice(uri.as_bytes());
    body.extend_from_slice(&w.to_le_bytes());
    body.extend_from_slice(&p.to_le_bytes());
    body.extend_from_slice(&deadline.to_le_bytes());
    body.extend_from_slice(&max_blocks.to_le_bytes());
    body.push(1);
    body.extend_from_slice(&attester.verifying_key().to_bytes());
    body.push(1);
    let wire = tx::sign(&Tx { chain_id, from: me, nonce, fee: min_fee, body: Body::Raw { kind: kind::POST_JOB, body } }, &poster)?;
    println!("{}", json!({"event": "post_job", "job_id": hex::encode(job_id), "tx": node.send_tx(&wire)?, "W": w.to_string(), "P": p.to_string(), "deadline_height": deadline}));
    Ok(())
}
