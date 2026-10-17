//! Published test vectors for the TypeScript wallet (chain/agent-types/vectors/*.json).
//!
//! `UPDATE_VECTORS=1 cargo test -p agent-types --test vectors` rewrites the files;
//! a plain run checks that the encoder still produces them byte for byte.

use agent_types::block::{encode_header_with_proof, tx_root};
use agent_types::tx::{deposit_message, order_job_id, order_message, pubkey_from_seed, result_message, sign_plain};
use agent_types::*;
use serde_json::{json, Value};

const CHAIN_ID: u32 = 0x5053_0001;

fn seed(i: u8) -> [u8; 32] {
    blake3_hash(&[b"pearl-sol:vector-seed", &[i]])
}

fn b58(k: &[u8]) -> String {
    bs58::encode(k).into_string()
}

fn sample_job() -> PostJob {
    let mut config = [0u8; 52];
    config[0..4].copy_from_slice(&1024u32.to_le_bytes());
    config[4..6].copy_from_slice(&32u16.to_le_bytes());
    PostJob {
        m: 64,
        n: 64,
        k: 1024,
        config,
        hash_a: [0xaa; 32],
        hash_b: [0xbb; 32],
        data_uri: b"ipfs://example-job".to_vec(),
        worker_pay: 5_000_000_000,
        protocol_fee: 1_000_000_000,
        deadline_height: 1000,
        max_blocks: 10,
        evaluators: vec![pubkey_from_seed(&seed(20)), pubkey_from_seed(&seed(21))],
        threshold: 1,
    }
}

fn job_json(p: &PostJob) -> Value {
    json!({
        "m": p.m, "n": p.n, "k": p.k,
        "config": hex::encode(p.config),
        "hash_a": hex::encode(p.hash_a), "hash_b": hex::encode(p.hash_b),
        "data_uri": String::from_utf8_lossy(&p.data_uri),
        "worker_pay": p.worker_pay.to_string(), "protocol_fee": p.protocol_fee.to_string(),
        "deadline_height": p.deadline_height, "max_blocks": p.max_blocks,
        "evaluators": p.evaluators.iter().map(|e| b58(e)).collect::<Vec<_>>(),
        "threshold": p.threshold,
    })
}

fn sigs_json(s: &[AttesterSig]) -> Value {
    Value::Array(s.iter().map(|a| json!({"pubkey": b58(&a.pubkey), "sig": hex::encode(a.sig)})).collect())
}

fn body_json(b: &TxBody) -> Value {
    match b {
        TxBody::Transfer { to, amount } => json!({"to": b58(to), "amount": amount.to_string()}),
        TxBody::RegisterAgent { name, uri } => {
            json!({"name": String::from_utf8_lossy(name), "uri": String::from_utf8_lossy(uri)})
        }
        TxBody::PostJob(p) => job_json(p),
        TxBody::SubmitResult { job_id, result_hash, uri } => json!({
            "job_id": hex::encode(job_id), "result_hash": hex::encode(result_hash),
            "uri": String::from_utf8_lossy(uri)}),
        TxBody::CompleteJob { job_id, sigs } => json!({"job_id": hex::encode(job_id), "sigs": sigs_json(sigs)}),
        TxBody::ExpireJob { job_id } => json!({"job_id": hex::encode(job_id)}),
        TxBody::BridgeUnlock { solana_nonce, recipient, amount_spl, job_order, sigs } => json!({
            "solana_nonce": solana_nonce, "recipient": b58(recipient), "amount_spl": amount_spl.to_string(),
            "job_order": job_order.as_ref().map(job_json), "sigs": sigs_json(sigs)}),
        TxBody::BridgeLock { solana_recipient, amount } => {
            json!({"solana_recipient": b58(solana_recipient), "amount": amount.to_string()})
        }
        TxBody::AdminConfig { key, value } => json!({"key": key, "value": hex::encode(value)}),
    }
}

fn tx_vector(name: &str, seed_i: u8, nonce: u64, fee: u64, body: TxBody) -> (Value, Tx) {
    let s = seed(seed_i);
    let tx = Tx::signed(&s, CHAIN_ID, nonce, fee, body);
    assert!(tx.verify_sig());
    assert_eq!(Tx::decode(&tx.encode()).unwrap(), tx);
    let v = json!({
        "name": name,
        "seed": hex::encode(s),
        "from": b58(&tx.from),
        "chain_id": CHAIN_ID,
        "nonce": nonce,
        "fee": fee.to_string(),
        "kind": tx.body.kind() as u8,
        "kind_name": tx.body.kind().name(),
        "body": body_json(&tx.body),
        "signing_bytes": hex::encode(tx.signing_bytes()),
        "framed_message": hex::encode(tx.framed_signing_message()),
        "sig": hex::encode(tx.sig),
        "tx": hex::encode(tx.encode()),
        "tx_hash": hex::encode(tx.hash()),
    });
    (v, tx)
}

fn signed_sig(seed_i: u8, msg: &[u8]) -> AttesterSig {
    let s = seed(seed_i);
    AttesterSig { pubkey: pubkey_from_seed(&s), sig: sign_plain(&s, msg) }
}

fn build() -> Vec<(&'static str, Value)> {
    let alice = pubkey_from_seed(&seed(1));
    let bob = pubkey_from_seed(&seed(2));
    let job = sample_job();
    let job_id = blake3_hash(&[&alice, &3u64.to_le_bytes()]);
    let result_hash = [0x5a; 32];
    let rmsg = result_message(&job_id, &bob, &result_hash);
    let dmsg = deposit_message(42, &alice, 1_000_000);
    // Storefront order: deadline is relative (blocks after the unlock height).
    let mut order_job = job.clone();
    order_job.deadline_height = 500;
    let omsg = order_message(43, &alice, 6_000_000, &order_job);
    let mut txs = Vec::new();
    let mut add = |v: (Value, Tx)| txs.push(v);
    add(tx_vector("transfer", 1, 0, 1000, TxBody::Transfer { to: bob, amount: 123_456_789 }));
    add(tx_vector(
        "register_agent",
        1,
        1,
        1000,
        TxBody::RegisterAgent { name: b"alice-agent".to_vec(), uri: b"https://example.com/agent.json".to_vec() },
    ));
    add(tx_vector("post_job", 1, 3, 1000, TxBody::PostJob(job.clone())));
    add(tx_vector(
        "submit_result",
        2,
        0,
        1000,
        TxBody::SubmitResult { job_id, result_hash, uri: b"ipfs://result".to_vec() },
    ));
    add(tx_vector(
        "complete_job",
        2,
        1,
        1000,
        TxBody::CompleteJob { job_id, sigs: vec![signed_sig(20, &rmsg)] },
    ));
    add(tx_vector("expire_job", 3, 0, 1000, TxBody::ExpireJob { job_id }));
    add(tx_vector(
        "bridge_unlock",
        4,
        0,
        0,
        TxBody::BridgeUnlock {
            solana_nonce: 42,
            recipient: alice,
            amount_spl: 1_000_000,
            job_order: None,
            sigs: vec![signed_sig(30, &dmsg), signed_sig(31, &dmsg)],
        },
    ));
    add(tx_vector(
        "bridge_unlock_order",
        4,
        1,
        0,
        TxBody::BridgeUnlock {
            solana_nonce: 43,
            recipient: alice,
            amount_spl: 6_000_000,
            job_order: Some(order_job.clone()),
            sigs: vec![signed_sig(30, &omsg)],
        },
    ));
    add(tx_vector(
        "bridge_lock",
        1,
        4,
        1000,
        TxBody::BridgeLock { solana_recipient: bob, amount: 500_000_000 },
    ));
    add(tx_vector(
        "admin_config",
        5,
        0,
        1000,
        TxBody::AdminConfig { key: 1, value: 2000u64.to_le_bytes().to_vec() },
    ));

    let tx_objs: Vec<Tx> = txs.iter().map(|(_, t)| t.clone()).collect();
    let tx_json: Vec<Value> = txs.into_iter().map(|(v, _)| v).collect();

    let msg_v = |name: &str, domain: &[u8], msg: &[u8], seed_i: u8| {
        json!({"name": name, "domain": String::from_utf8_lossy(domain), "message": hex::encode(msg),
               "signing": "raw ed25519 over message (no framing)",
               "seed": hex::encode(seed(seed_i)), "pubkey": b58(&pubkey_from_seed(&seed(seed_i))),
               "sig": hex::encode(sign_plain(&seed(seed_i), msg))})
    };
    let mut order_v = msg_v("deposit_order", DOMAIN_DEPOSIT_ORDER, &omsg, 30);
    order_v["post_job_body"] = json!(hex::encode(order_job.encode()));
    order_v["post_job_body_sha256"] = json!(hex::encode(order_job.body_hash()));
    order_v["solana_nonce"] = json!(43);
    order_v["job_id"] = json!(hex::encode(order_job_id(43)));
    order_v["job_id_rule"] = json!("sha256(b\"pearl-sol:order-job:v1\" || solana_nonce u64 LE)");
    order_v["deadline_rule"] = json!("relative: deadline_height = unlock block height + body.deadline_height");
    let messages = json!([
        msg_v("deposit", DOMAIN_DEPOSIT, &dmsg, 30),
        order_v,
        msg_v("result", DOMAIN_RESULT, &rmsg, 20),
        {"name": "job_id", "poster": b58(&alice), "poster_nonce": 3, "job_id": hex::encode(job_id),
         "rule": "blake3(poster || nonce u64 LE)"},
        {"name": "reserved_addresses", "escrow": b58(&escrow_address()), "pool": b58(&pool_address())},
    ]);

    let proof = b"example proof bytes".to_vec();
    let header = Header {
        version: 1,
        height: 12,
        prev_hash: blake3_hash(&[b"parent"]),
        timestamp_ms: 1_760_000_000_000,
        target_bits: 0x207fffff,
        job_id,
        miner: bob,
        tx_root: tx_root(&tx_objs[..2]),
        state_root: [0x11; 32],
        withdraw_root: [0x22; 32],
        proof_hash: agent_types::block::proof_hash(&proof),
        nonce: 77,
    };
    let block = Block { header, proof: proof.clone(), txs: tx_objs[..2].to_vec() };
    assert_eq!(Block::decode(&block.encode()).unwrap(), block);
    let header_v = json!({
        "fields": {
            "version": header.version, "height": header.height, "prev_hash": hex::encode(header.prev_hash),
            "timestamp_ms": header.timestamp_ms, "target_bits": header.target_bits,
            "job_id": hex::encode(header.job_id), "miner": b58(&header.miner),
            "tx_root": hex::encode(header.tx_root), "state_root": hex::encode(header.state_root),
            "withdraw_root": hex::encode(header.withdraw_root), "proof_hash": hex::encode(header.proof_hash),
            "nonce": header.nonce,
        },
        "header": hex::encode(header.encode()),
        "block_hash": hex::encode(header.hash()),
        "pow_digest": hex::encode(header.pow_digest()),
        "header_with_proof": hex::encode(encode_header_with_proof(&header, &proof)),
        "block": hex::encode(block.encode()),
        "block_tx_hashes": tx_objs[..2].iter().map(|t| hex::encode(t.hash())).collect::<Vec<_>>(),
    });
    vec![("tx.json", Value::Array(tx_json)), ("messages.json", messages), ("header.json", header_v)]
}

#[test]
fn vectors_match_files() {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("vectors");
    for (name, v) in build() {
        let text = serde_json::to_string_pretty(&v).unwrap() + "\n";
        let path = dir.join(name);
        if std::env::var("UPDATE_VECTORS").is_ok() || !path.exists() {
            std::fs::write(&path, &text).unwrap();
        } else {
            let on_disk = std::fs::read_to_string(&path).unwrap();
            assert_eq!(on_disk, text, "{name} drifted; rerun with UPDATE_VECTORS=1 if intended");
        }
    }
}
