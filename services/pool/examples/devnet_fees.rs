//! Devnet helper for the live pool check: blocks on a devnet without jobs pay fees only,
//! so this funds a throwaway sponsor key with an attested BridgeUnlock (the devnet
//! attester key signs the plain deposit message, docs/RPC.md) and then sends Transfer
//! txs with a large fee, one every `interval` ms, so the blocks the pool mines carry
//! a real reward. Devnet only (the attester key is the localnet one).
//!
//!   cargo run -p pearl-pool --example devnet_fees -- <rpc> <attester-key> <solana_nonce> <amount_spl> <fee> <count> <interval_ms>

use std::time::Duration;

use anyhow::{Context, Result};
use ed25519_dalek::{Signer, SigningKey};

use pearl_node::tx::{self, Body, Tx};
use pearl_node::{b58, read_key_file, HttpNode, NodeApi};

fn main() -> Result<()> {
    let a: Vec<String> = std::env::args().skip(1).collect();
    anyhow::ensure!(a.len() == 7, "usage: devnet_fees <rpc> <attester-key> <solana_nonce> <amount_spl> <fee> <count> <interval_ms>");
    let node = HttpNode::new(&a[0]);
    let attester = read_key_file(std::path::Path::new(&a[1]))?;
    let (sol_nonce, amount_spl, fee, count, interval): (u64, u64, u64, u64, u64) =
        (a[2].parse()?, a[3].parse()?, a[4].parse()?, a[5].parse()?, a[6].parse()?);
    let info = node.call("getChainInfo", serde_json::json!({}))?;
    let chain_id = info["chain_id"].as_u64().context("chain_id")? as u32;
    let sponsor = SigningKey::from_bytes(blake3::hash(format!("pearl-sol pool devnet sponsor {sol_nonce}").as_bytes()).as_bytes());
    let me = sponsor.verifying_key().to_bytes();
    println!("{}", serde_json::json!({"event": "sponsor", "key": b58(&me), "chain_id": chain_id}));

    let dep = node.call("getDeposit", serde_json::json!({"solana_nonce": sol_nonce}))?;
    if dep["processed"].as_bool() != Some(true) {
        let mut msg = b"pearl-sol:deposit:v1".to_vec();
        msg.extend_from_slice(&sol_nonce.to_le_bytes());
        msg.extend_from_slice(&me);
        msg.extend_from_slice(&amount_spl.to_le_bytes());
        let sig = attester.sign(&msg).to_bytes();
        let mut body = Vec::new();
        body.extend_from_slice(&sol_nonce.to_le_bytes());
        body.extend_from_slice(&me);
        body.extend_from_slice(&amount_spl.to_le_bytes());
        body.push(0); // job_order
        body.push(1);
        body.extend_from_slice(&attester.verifying_key().to_bytes());
        body.extend_from_slice(&sig);
        let n = node.get_account(&me)?.nonce;
        let w = tx::sign(&Tx { chain_id, from: me, nonce: n, fee: 0, body: Body::Raw { kind: tx::kind::BRIDGE_UNLOCK, body } }, &sponsor)?;
        println!("{}", serde_json::json!({"event": "bridge_unlock", "tx": node.send_tx(&w)?, "amount_spl": amount_spl}));
    }
    while node.get_account(&me)?.balance == 0 {
        std::thread::sleep(Duration::from_millis(500));
    }
    let sink = [0x5c; 32];
    let mut sent = 0;
    while sent < count {
        let acct = node.get_account(&me)?;
        let w = tx::sign(&Tx { chain_id, from: me, nonce: acct.nonce, fee, body: Body::Transfer { to: sink, amount: 1 } }, &sponsor)?;
        match node.send_tx(&w) {
            Ok(h) => {
                sent += 1;
                println!("{}", serde_json::json!({"event": "fee_tx", "tx": h, "fee": fee, "nonce": acct.nonce, "balance": acct.balance.to_string()}));
                // Wait for inclusion so the next nonce is right.
                while node.get_account(&me)?.nonce == acct.nonce {
                    std::thread::sleep(Duration::from_millis(200));
                }
            }
            Err(e) => println!("{}", serde_json::json!({"event": "fee_tx_failed", "error": format!("{e:#}")})),
        }
        std::thread::sleep(Duration::from_millis(interval));
    }
    Ok(())
}
