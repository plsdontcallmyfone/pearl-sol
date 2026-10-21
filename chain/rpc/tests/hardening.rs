//! Node API admission limits (security pass 2026-10-21), on `rpc::Core` directly in the
//! deterministic runtime: mempool admission (funding, BridgeUnlock attester check, admin
//! guard), unsolicited blob pushes, and the RPC batch cap.

use agent_types::tx::{deposit_message, pubkey_from_seed, sign_plain};
use agent_types::*;
use blobs::{BlobConfig, BlobStore};
use commonware_runtime::{deterministic, Runner as _, Supervisor as _};
use consensus_pow::{Chain, Genesis, JobChoice, MockVerifier};
use rpc::Core;
use serde_json::json;
use state::config::{keys, Config};

const CHAIN: u32 = 11;
const T0: u64 = 1_760_000_000_000;

fn seed(i: u8) -> [u8; 32] {
    [i; 32]
}
fn pk(i: u8) -> Pubkey {
    pubkey_from_seed(&seed(i))
}

fn genesis() -> Genesis {
    let mut c = Config::defaults(pk(1));
    c.finality_k = 3;
    c.target_block_time_ms = 1000;
    c.attesters = vec![pk(50)];
    c.attester_threshold = 1;
    c.min_fee = 10;
    Genesis { chain_id: CHAIN, timestamp_ms: T0, target_bits: 0x207fffff, config: c }
}

type K = Core<deterministic::Context, MockVerifier>;

fn unlock(relayer: u8, rnonce: u64, nonce: u64, to: Pubkey, amount_spl: u64, signer: u8) -> Tx {
    let msg = deposit_message(nonce, &to, amount_spl);
    let sigs = vec![AttesterSig { pubkey: pk(signer), sig: sign_plain(&seed(signer), &msg) }];
    Tx::signed(&seed(relayer), CHAIN, rnonce, 0, TxBody::BridgeUnlock { solana_nonce: nonce, recipient: to, amount_spl, job_order: None, sigs })
}

/// Mine one fallback block carrying the mempool's txs.
async fn mine(k: &mut K, now: u64) {
    let (_, t) = k.template(pk(9), JobChoice::Fallback, now).await.unwrap();
    let mut h = t.header;
    let proof = MockVerifier::proof_for(&h);
    h.proof_hash = block::proof_hash(&proof);
    let b = Block { header: h, proof, txs: t.txs.clone(), coinbase: Some(t.summary.coinbase) };
    let r = k.add_block(b, now, false).await;
    assert!(r.accepted && r.tip, "{:?}", r.reason);
}

#[test]
fn mempool_admission() {
    deterministic::Runner::default().start(|ctx| async move {
        let chain = Chain::open(ctx.child("c"), "n", genesis(), MockVerifier).await.unwrap();
        let mut k: K = Core::new(chain);
        // 1. a key with no funds cannot pool txs (before: accepted, never mined, pool fills for free)
        let t = Tx::signed(&seed(7), CHAIN, 0, 10, TxBody::Transfer { to: pk(8), amount: 1 });
        let e = k.add_tx(t, false).await.unwrap_err();
        assert!(e.contains("cannot pay"), "{e}");
        // 2. a BridgeUnlock pays no fee: one signed by a non-attester is refused at admission
        let e = k.add_tx(unlock(7, 0, 1, pk(2), 5_000, 66), false).await.unwrap_err();
        assert!(e.contains("signer not in the set"), "{e}");
        // ... an attester-signed one is pooled and mined
        k.add_tx(unlock(7, 0, 1, pk(2), 5_000, 50), false).await.unwrap();
        mine(&mut k, T0 + 1000).await;
        assert!(k.mempool.is_empty());
        // ... and once processed, a second unlock of the same deposit is refused at admission
        let e = k.add_tx(unlock(7, 1, 1, pk(2), 5_000, 50), false).await.unwrap_err();
        assert!(e.contains("already processed"), "{e}");
        // 3. the funded account (5,000,000 base units) pools txs whose fees it can pay
        let t = Tx::signed(&seed(2), CHAIN, 0, 10, TxBody::Transfer { to: pk(8), amount: 1 });
        k.add_tx(t, false).await.unwrap();
        let big = Tx::signed(&seed(2), CHAIN, 1, 5_000_000, TxBody::Transfer { to: pk(8), amount: 1 });
        let e = k.add_tx(big, false).await.unwrap_err();
        assert!(e.contains("cannot pay"), "{e}");
    });
}

#[test]
fn admin_cannot_brick_the_chain_over_rpc() {
    deterministic::Runner::default().start(|ctx| async move {
        let chain = Chain::open(ctx.child("c"), "n", genesis(), MockVerifier).await.unwrap();
        let mut k: K = Core::new(chain);
        // fund the admin (pk 1) with 1,000,000 base units
        k.add_tx(unlock(7, 0, 1, pk(1), 1_000, 50), false).await.unwrap();
        mine(&mut k, T0 + 1000).await;
        let set = |v: Vec<u8>, key: u8, nonce: u64| Tx::signed(&seed(1), CHAIN, nonce, 10, TxBody::AdminConfig { key, value: v });
        let e = k.add_tx(set(u64::MAX.to_le_bytes().to_vec(), keys::MIN_FEE, 0), false).await.unwrap_err();
        assert!(e.contains("more than the admin can pay"), "{e}");
        let e = k.add_tx(set(vec![0; 32], keys::ADMIN, 0), false).await.unwrap_err();
        assert!(e.contains("zero admin key"), "{e}");
        // an ordinary change still goes through
        k.add_tx(set(20u64.to_le_bytes().to_vec(), keys::MIN_FEE, 0), false).await.unwrap();
    });
}

#[test]
fn unsolicited_blob_pushes_fill_at_most_half() {
    deterministic::Runner::default().start(|ctx| async move {
        let chain = Chain::open(ctx.child("c"), "n", genesis(), MockVerifier).await.unwrap();
        let cfg = BlobConfig { max_blob_bytes: 1000, max_total_bytes: 10_000, peer_burst_bytes: 1 << 30, peer_rate_bytes_per_s: 1 << 30, ..Default::default() };
        let store = BlobStore::open(ctx.child("b"), "blobs", cfg, 0).await.unwrap();
        let mut k: K = Core::new(chain).with_blobs(store);
        for i in 0..20u8 {
            assert!(k.receive_blob(b"peer", &[i; 1000], T0).await);
        }
        let total = k.blobs.as_ref().unwrap().total_bytes();
        assert!(total <= 5_000, "pushed blobs hold {total} of 10000 bytes");
        // an RPC put (rate limited per node, and the path job posters use) still fits
        let (id, new) = k.put_blob_opt(&[200u8; 1000], T0, false).await.unwrap();
        assert!(new && k.blobs.as_ref().unwrap().has(&id).is_some());
    });
}

#[test]
fn get_blob_of_unknown_id_reuses_the_reference_scan() {
    deterministic::Runner::default().start(|ctx| async move {
        let chain = Chain::open(ctx.child("c"), "n", genesis(), MockVerifier).await.unwrap();
        let store = BlobStore::open(ctx.child("b"), "blobs", BlobConfig::default(), 0).await.unwrap();
        let mut k: K = Core::new(chain).with_blobs(store);
        assert!(k.refs_cache.is_none());
        for i in 0..50u8 {
            let r = k.dispatch("getBlob", &json!({"blob_id": hex::encode([i; 32])}), T0).await.unwrap();
            assert!(r.is_null());
        }
        // one scan for this tip, kept for every later miss
        let tip = k.chain.tip().hash;
        assert_eq!(k.refs_cache.as_ref().map(|(h, _)| *h), Some(tip));
    });
}

#[test]
fn batch_cap() {
    // the cap is checked before any call reaches the node actor
    let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
    rt.block_on(async {
        let (tx, _rx) = futures::channel::mpsc::channel(1);
        let reqs: Vec<_> = (0..rpc::server::MAX_BATCH + 1).map(|i| json!({"jsonrpc": "2.0", "id": i, "method": "getChainInfo"})).collect();
        let v = rpc::server::handle_body(tx, serde_json::Value::Array(reqs)).await;
        assert_eq!(v["error"]["code"], json!(-32600), "{v}");
    });
}
