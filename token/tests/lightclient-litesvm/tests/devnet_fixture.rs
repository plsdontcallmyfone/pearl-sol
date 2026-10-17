//! Replays a REAL agent-chain devnet (fixtures/devnet.json, harvested with
//! `services/header-relay harvest` from a 1-node devnet seeded by `header-relay seed`):
//! genesis checkpoint, 15 headers (fallback blocks and one job block on a PostJob),
//! the node's own withdrawal MMR proof, released by proof through the bridge.

use base64::Engine;
use lightclient::client;
use lightclient_litesvm_tests::*;
use pearl_verify_sbf as pv;
use serde_json::Value;

fn fixture() -> Value {
    let p = concat!(env!("CARGO_MANIFEST_DIR"), "/fixtures/devnet.json");
    serde_json::from_str(&std::fs::read_to_string(p).expect("fixtures/devnet.json")).unwrap()
}

fn b64(s: &str) -> Vec<u8> {
    base64::engine::general_purpose::STANDARD.decode(s).unwrap()
}

#[test]
fn real_devnet_chain_job_block_and_withdrawal_by_proof() {
    let f = fixture();
    let fp = &f["params"];
    let n = |k: &str| fp[k].as_u64().unwrap();
    let mut p = params(n("finality_k") as u32);
    p.chain_id = f["chain_id"].as_u64().unwrap() as u32;
    p.target_block_time_ms = n("target_block_time_ms");
    p.half_life_ms = n("half_life_ms");
    p.pow_limit_bits = n("pow_limit_bits") as u32;
    p.min_difficulty_bits = n("min_difficulty_bits") as u32;
    p.max_future_ms = n("max_future_ms");
    p.fallback_m = n("fallback_m") as u32;
    p.fallback_n = n("fallback_n") as u32;
    p.fallback_config = hex::decode(fp["fallback_config"].as_str().unwrap()).unwrap().try_into().unwrap();
    let hs = f["headers"].as_array().unwrap();
    let blk = |i: usize| {
        let h = &hs[i];
        let bytes: [u8; 256] = hex::decode(h["header"].as_str().unwrap()).unwrap().try_into().unwrap();
        let cw = pv::U256::from_str_radix(h["cumulative_work"].as_str().unwrap(), 16).unwrap();
        let ts = u64::from_le_bytes(bytes[44..52].try_into().unwrap());
        Blk::from_wire(bytes, b64(h["proof"].as_str().unwrap()), cw, vec![ts])
    };
    let g = blk(0);
    let last_ts = blk(hs.len() - 1).h.timestamp_ms;
    let mut env = Env::new_at(p, &g, (last_ts / 1000) as i64 + 5);
    let mut job_blocks = 0;
    for i in 1..hs.len() {
        let b = blk(i);
        assert_eq!(b.h.prev_hash, blk(i - 1).hash);
        env.relay(&b).unwrap_or_else(|e| panic!("height {i}: {e:?}"));
        assert_eq!(env.status(&b.hash), lightclient::STATUS_VERIFIED);
        if !b.h.is_fallback() {
            job_blocks += 1;
        }
        // register jobs posted in this block, like the relay does
        let txs: Vec<Vec<u8>> = hs[i]["txs"].as_array().unwrap().iter().map(|t| b64(t.as_str().unwrap())).collect();
        let (root, paths) = tx_root(&txs);
        assert_eq!(root, b.h.tx_root, "tx_root rebuilt from the block's txs");
        for (ti, t) in txs.iter().enumerate() {
            let v = pv::chaintx::TxView::any_chain(t).unwrap();
            if v.kind() == pv::chaintx::KIND_POST_JOB {
                let jc = pv::chaintx::job_from_tx(&v, |parts| {
                    use sha2::Digest;
                    let mut h = sha2::Sha256::new();
                    for x in parts {
                        h.update(x);
                    }
                    h.finalize().into()
                })
                .unwrap();
                let pk = env.payer.insecure_clone();
                env.send(&[client::register_job(solana_sdk::signer::Signer::pubkey(&pk), &b.hash, jc.job_id, t.clone(), paths[ti].clone())], &[]).unwrap();
            }
        }
    }
    assert_eq!(job_blocks, 1, "the fixture has one live job block");
    let c = env.config();
    assert_eq!(c.tip_hash, blk(hs.len() - 1).hash);
    assert_eq!(pv::difficulty::u256_from_le(&c.tip_work), blk(hs.len() - 1).cum_work, "cumulative work equals the node's");
    // withdrawal: the node's own proof against withdraw_root of its final header
    let w = &f["withdrawals"][0];
    let root_hash: [u8; 32] = hex::decode(w["root_block_hash"].as_str().unwrap()).unwrap().try_into().unwrap();
    env.mark_final(&root_hash).unwrap();
    let bs = env.setup_bridge(1_000_000_000, bridge::MODE_PROOF);
    let entry = hex::decode(w["entry"].as_str().unwrap()).unwrap();
    let to = solana_sdk::pubkey::Pubkey::new_from_array(entry[40..72].try_into().unwrap());
    let to_t = env.create_ata(&to, &bs.mint);
    let loc = w["location"].as_u64().unwrap();
    let proof = b64(w["proof"].as_str().unwrap());
    let nonce = u64::from_le_bytes(entry[0..8].try_into().unwrap());
    let mut forged = proof.clone();
    let last = forged.len() - 1;
    forged[last] ^= 1;
    let ix = env.release_proof_ix(&bs, nonce, &entry, loc, forged, &root_hash, to_t);
    assert!(custom(u32::from(bridge::BridgeError::BadProof))(&env.send(&[ix], &[]).unwrap_err()));
    let ix = env.release_proof_ix(&bs, nonce, &entry, loc, proof, &root_hash, to_t);
    let cu = env.send(&[ix], &[]).unwrap();
    let amount = u64::from_le_bytes(entry[72..80].try_into().unwrap());
    assert_eq!(env.token_balance(&to_t), amount);
    println!("real devnet: {} headers verified (1 job block), withdrawal {nonce} of {amount} released by proof ({cu} CU)", hs.len() - 1);
}
