//! Multi-node tests: real node actors on Commonware's deterministic runtime and
//! simulated p2p network. Blocks are mined through the JSON-RPC dispatcher
//! (getBlockTemplate / submitBlock) with MockVerifier proofs.

use std::time::{Duration, UNIX_EPOCH};

use agent_node::node::{run, ActorConfig};
use agent_types::tx::{deposit_message, pubkey_from_seed, sign_plain};
use agent_types::*;
use base64::Engine as _;
use commonware_cryptography::{ed25519, Signer as _};
use commonware_p2p::simulated::{Config as NetConfig, Link, Network, Oracle};
use commonware_p2p::Manager as _;
use commonware_runtime::{deterministic, Clock, Runner as _, Spawner as _, Supervisor as _};
use commonware_utils::ordered::Set;
use consensus_pow::{Chain, Genesis, MockVerifier};
use futures::channel::{mpsc, oneshot};
use futures::SinkExt;
use rpc::{ApiRequest, Core};
use serde_json::{json, Value};
use state::config::Config;

const CHAIN: u32 = 11;
const T0: u64 = 1_760_000_000_000;

fn seed(i: u8) -> [u8; 32] {
    [i; 32]
}
fn pk(i: u8) -> Pubkey {
    pubkey_from_seed(&seed(i))
}

fn genesis(k: u32) -> Genesis {
    let mut c = Config::defaults(pk(1));
    c.finality_k = k;
    c.target_block_time_ms = 1000;
    c.attesters = vec![pk(50)];
    c.attester_threshold = 1;
    c.min_fee = 10;
    Genesis { chain_id: CHAIN, timestamp_ms: T0, target_bits: 0x207fffff, config: c }
}

#[derive(Clone)]
struct Api(mpsc::Sender<ApiRequest>);

impl Api {
    async fn call(&self, method: &str, params: Value) -> Value {
        let (tx, rx) = oneshot::channel();
        self.0.clone().send(ApiRequest::Call { method: method.into(), params, reply: tx }).await.unwrap();
        rx.await.unwrap().unwrap_or_else(|e| panic!("{method}: {}", e.message))
    }

    async fn tip(&self) -> (String, u64) {
        let v = self.call("getChainInfo", json!({})).await;
        (v["tip"].as_str().unwrap().to_string(), v["height"].as_u64().unwrap())
    }

    /// Mine one fallback block on this node's tip.
    async fn mine(&self, miner: u8) -> Value {
        let t = self.call("getBlockTemplate", json!({"miner": bs58::encode(pk(miner)).into_string(), "job_id": "fallback"})).await;
        let mut h = Header::decode(&hex::decode(t["header"].as_str().unwrap()).unwrap()).unwrap();
        let proof = MockVerifier::proof_for(&h);
        h.proof_hash = block::proof_hash(&proof);
        let r = self
            .call(
                "submitBlock",
                json!({"header": hex::encode(h.encode()), "proof": base64::engine::general_purpose::STANDARD.encode(&proof),
                       "template_id": t["template_id"]}),
            )
            .await;
        assert_eq!(r["accepted"], json!(true), "{r}");
        r
    }

    async fn try_call(&self, method: &str, params: Value) -> Result<Value, String> {
        let (tx, rx) = oneshot::channel();
        self.0.clone().send(ApiRequest::Call { method: method.into(), params, reply: tx }).await.unwrap();
        rx.await.unwrap().map_err(|e| e.message)
    }

    /// Mine one block with a template choice ("fallback", "auto" = omit, or a job id).
    async fn mine_with(&self, miner: u8, job: Option<&str>) -> (Value, Value) {
        let mut q = json!({"miner": bs58::encode(pk(miner)).into_string()});
        if let Some(j) = job {
            q["job_id"] = json!(j);
        }
        let t = self.call("getBlockTemplate", q).await;
        let mut h = Header::decode(&hex::decode(t["header"].as_str().unwrap()).unwrap()).unwrap();
        let proof = MockVerifier::proof_for(&h);
        h.proof_hash = block::proof_hash(&proof);
        let r = self
            .call(
                "submitBlock",
                json!({"header": hex::encode(h.encode()), "proof": base64::engine::general_purpose::STANDARD.encode(&proof),
                       "template_id": t["template_id"]}),
            )
            .await;
        assert_eq!(r["accepted"], json!(true), "{r}");
        (t, r)
    }

    async fn send(&self, tx: &Tx) -> Result<Value, String> {
        self.try_call("sendTx", json!({"tx": base64::engine::general_purpose::STANDARD.encode(tx.encode())})).await
    }

    async fn balance(&self, who: &Pubkey) -> u64 {
        let v = self.call("getAccount", json!({"account": bs58::encode(who).into_string()})).await;
        v["balance"].as_str().unwrap().parse().unwrap()
    }
}

/// Blob grace period in the test nodes (blocks).
const BLOB_GRACE: u64 = 20;

const LINK: Link = Link { latency: Duration::from_millis(20), jitter: Duration::from_millis(5), success_rate: commonware_utils::probability!(1.0) };

struct Net {
    oracle: Oracle<ed25519::PublicKey, deterministic::Context>,
    keys: Vec<ed25519::PublicKey>,
    apis: Vec<Api>,
}

impl Net {
    async fn link(&self, a: usize, b: usize) {
        let _ = self.oracle.add_link(self.keys[a].clone(), self.keys[b].clone(), LINK).await;
        let _ = self.oracle.add_link(self.keys[b].clone(), self.keys[a].clone(), LINK).await;
    }
    async fn unlink(&self, a: usize, b: usize) {
        let _ = self.oracle.remove_link(self.keys[a].clone(), self.keys[b].clone()).await;
        let _ = self.oracle.remove_link(self.keys[b].clone(), self.keys[a].clone()).await;
    }
}

async fn setup(ctx: &deterministic::Context, n: usize, k: u32, linked: bool) -> Net {
    let (network, oracle) = Network::new(
        ctx.child("network"),
        NetConfig { max_size: 8 << 20, max_peers_per_set: std::num::NonZeroUsize::new(n).unwrap(), disconnect_on_block: false, tracked_peer_sets: std::num::NonZeroUsize::new(3).unwrap() },
    );
    network.start();
    let signers: Vec<ed25519::PrivateKey> = (0..n).map(|i| ed25519::PrivateKey::from_seed(100 + i as u64)).collect();
    let keys: Vec<ed25519::PublicKey> = signers.iter().map(|s| s.public_key()).collect();
    let mut manager = oracle.manager();
    let _ = manager.track(0, Set::try_from(keys.clone()).unwrap());
    let mut apis = Vec::new();
    for (i, key) in keys.iter().enumerate() {
        let ctl = oracle.control(key.clone());
        let q = net::quota();
        let ch = net::Channels {
            blocks: ctl.clone().register(net::CH_BLOCKS, q).await.unwrap(),
            txs: ctl.clone().register(net::CH_TXS, q).await.unwrap(),
            tips: ctl.clone().register(net::CH_TIPS, q).await.unwrap(),
            headers: ctl.clone().register(net::CH_HEADERS, q).await.unwrap(),
            bodies: ctl.clone().register(net::CH_BODIES, q).await.unwrap(),
            blobs: ctl.clone().register(net::CH_BLOBS, q).await.unwrap(),
            blob_fetch: ctl.clone().register(net::CH_BLOB_FETCH, q).await.unwrap(),
        };
        let prefix = format!("node{i}");
        let chain = Chain::open(ctx.child("chain").with_attribute("node", i), &prefix, genesis(k), MockVerifier).await.unwrap();
        let bcfg = blobs::BlobConfig { grace_blocks: BLOB_GRACE, ..Default::default() };
        let store = blobs::BlobStore::open(ctx.child("blobs").with_attribute("node", i), &format!("{prefix}-blobs"), bcfg, 0).await.unwrap();
        let core = Core::new(chain).with_blobs(store);
        let (tx, rx) = mpsc::channel(256);
        let me = key.clone();
        let provider = oracle.manager();
        let blocker = ctl.clone();
        ctx.child("node").with_attribute("node", i).spawn(move |c| async move {
            run(c, core, me, provider, blocker, ch, rx, ActorConfig { tip_interval: Duration::from_millis(500), ..Default::default() }).await;
        });
        apis.push(Api(tx));
    }
    let net = Net { oracle, keys, apis };
    if linked {
        for a in 0..n {
            for b in a + 1..n {
                net.link(a, b).await;
            }
        }
    }
    net
}

fn runner(seed: u64) -> deterministic::Runner {
    deterministic::Runner::new(
        deterministic::Config::new()
            .with_seed(seed)
            .with_start_time(UNIX_EPOCH + Duration::from_millis(T0 + 1000))
            .with_timeout(Some(Duration::from_secs(3600))),
    )
}

async fn wait_tips_equal(ctx: &deterministic::Context, apis: &[Api], secs: u64) -> (String, u64) {
    for _ in 0..secs * 10 {
        let mut tips = Vec::new();
        for a in apis {
            tips.push(a.tip().await);
        }
        if tips.iter().all(|t| *t == tips[0]) {
            return tips[0].clone();
        }
        ctx.sleep(Duration::from_millis(100)).await;
    }
    let mut tips = Vec::new();
    for a in apis {
        tips.push(a.tip().await);
    }
    panic!("tips did not converge: {tips:?}");
}

#[test]
fn three_nodes_gossip_blocks_and_txs() {
    runner(1).start(|ctx| async move {
        let net = setup(&ctx, 3, 5, true).await;
        ctx.sleep(Duration::from_secs(1)).await;
        for _ in 0..3 {
            net.apis[0].mine(9).await;
            ctx.sleep(Duration::from_secs(1)).await;
        }
        let (_, h) = wait_tips_equal(&ctx, &net.apis, 20).await;
        assert_eq!(h, 3);
        // a tx sent to node 1 reaches node 0's mempool and is mined there
        let msg = deposit_message(1, &pk(2), 77);
        let sig = AttesterSig { pubkey: pk(50), sig: sign_plain(&seed(50), &msg) };
        let tx = Tx::signed(&seed(200), CHAIN, 0, 0, TxBody::BridgeUnlock { solana_nonce: 1, recipient: pk(2), amount_spl: 77, job_order: None, sigs: vec![sig] });
        net.apis[1].call("sendTx", json!({"tx": base64::engine::general_purpose::STANDARD.encode(tx.encode())})).await;
        ctx.sleep(Duration::from_secs(1)).await;
        assert_eq!(net.apis[0].call("getMempool", json!({})).await["size"], json!(1));
        net.apis[0].mine(9).await;
        wait_tips_equal(&ctx, &net.apis, 20).await;
        assert_eq!(net.apis[2].balance(&pk(2)).await, 77_000);
        let d = net.apis[2].call("getDeposit", json!({"solana_nonce": 1})).await;
        assert_eq!(d["processed"], json!(true));
        assert_eq!(net.apis[1].call("getMempool", json!({})).await["size"], json!(0));
    });
}

#[test]
fn fork_race_resolves_to_one_tip() {
    runner(2).start(|ctx| async move {
        let net = setup(&ctx, 3, 10, true).await;
        ctx.sleep(Duration::from_secs(1)).await;
        net.apis[0].mine(3).await;
        wait_tips_equal(&ctx, &net.apis, 20).await;
        // nodes 0 and 1 mine at the same height before hearing of each other
        net.unlink(0, 1).await;
        net.unlink(0, 2).await;
        net.unlink(1, 2).await;
        net.apis[0].mine(3).await;
        net.apis[1].mine(4).await;
        let (t0, _) = net.apis[0].tip().await;
        let (t1, _) = net.apis[1].tip().await;
        assert_ne!(t0, t1);
        for a in 0..3 {
            for b in a + 1..3 {
                net.link(a, b).await;
            }
        }
        ctx.sleep(Duration::from_secs(3)).await;
        // node 2 extends whichever it has; everyone follows the heavier chain
        net.apis[2].mine(5).await;
        let (_, h) = wait_tips_equal(&ctx, &net.apis, 30).await;
        assert_eq!(h, 3);
    });
}

#[test]
fn partition_then_heal_reorgs_within_k() {
    runner(3).start(|ctx| async move {
        let net = setup(&ctx, 3, 10, true).await;
        ctx.sleep(Duration::from_secs(1)).await;
        net.apis[0].mine(3).await;
        wait_tips_equal(&ctx, &net.apis, 20).await;
        // fund an account before the split
        let msg = deposit_message(5, &pk(2), 1000);
        let sig = AttesterSig { pubkey: pk(50), sig: sign_plain(&seed(50), &msg) };
        let fund = Tx::signed(&seed(200), CHAIN, 0, 0, TxBody::BridgeUnlock { solana_nonce: 5, recipient: pk(2), amount_spl: 1000, job_order: None, sigs: vec![sig] });
        net.apis[0].call("sendTx", json!({"tx": base64::engine::general_purpose::STANDARD.encode(fund.encode())})).await;
        ctx.sleep(Duration::from_millis(500)).await;
        net.apis[0].mine(3).await;
        wait_tips_equal(&ctx, &net.apis, 20).await;
        // partition {0} | {1, 2}
        net.unlink(0, 1).await;
        net.unlink(0, 2).await;
        // the same coins are spent differently on each side
        let a = Tx::signed(&seed(2), CHAIN, 0, 10, TxBody::Transfer { to: pk(3), amount: 500_000 });
        let b = Tx::signed(&seed(2), CHAIN, 0, 10, TxBody::Transfer { to: pk(4), amount: 500_000 });
        net.apis[0].call("sendTx", json!({"tx": base64::engine::general_purpose::STANDARD.encode(a.encode())})).await;
        net.apis[1].call("sendTx", json!({"tx": base64::engine::general_purpose::STANDARD.encode(b.encode())})).await;
        for _ in 0..2 {
            net.apis[0].mine(3).await;
            ctx.sleep(Duration::from_secs(1)).await;
        }
        for _ in 0..4 {
            net.apis[1].mine(4).await;
            ctx.sleep(Duration::from_secs(1)).await;
        }
        assert_eq!(net.apis[0].balance(&pk(3)).await, 500_000);
        // heal: node 0 learns the heavier branch from tip announcements and reorgs
        net.link(0, 1).await;
        net.link(0, 2).await;
        let (_, h) = wait_tips_equal(&ctx, &net.apis, 60).await;
        assert_eq!(h, 6);
        assert_eq!(net.apis[0].balance(&pk(3)).await, 0, "the payment on the losing branch is gone");
        assert_eq!(net.apis[0].balance(&pk(4)).await, 500_000);
        // the orphaned double spend is not minable any more
        net.apis[0].mine(3).await;
        wait_tips_equal(&ctx, &net.apis, 20).await;
        assert_eq!(net.apis[0].balance(&pk(3)).await, 0);
    });
}

#[test]
fn partition_beyond_k_is_refused() {
    runner(4).start(|ctx| async move {
        let net = setup(&ctx, 3, 3, true).await;
        ctx.sleep(Duration::from_secs(1)).await;
        net.apis[0].mine(3).await;
        wait_tips_equal(&ctx, &net.apis, 20).await;
        net.unlink(0, 1).await;
        net.unlink(0, 2).await;
        for _ in 0..6 {
            net.apis[0].mine(3).await;
            ctx.sleep(Duration::from_secs(1)).await;
        }
        for _ in 0..8 {
            net.apis[1].mine(4).await;
            ctx.sleep(Duration::from_secs(1)).await;
        }
        net.link(0, 1).await;
        net.link(0, 2).await;
        ctx.sleep(Duration::from_secs(10)).await;
        let (t0, h0) = net.apis[0].tip().await;
        let (t1, h1) = net.apis[1].tip().await;
        let (t2, _) = net.apis[2].tip().await;
        assert_eq!((h0, h1), (7, 9));
        assert_ne!(t0, t1, "node 0 refuses a reorg deeper than K");
        assert_eq!(t1, t2);
        // both sides keep extending their own chain
        net.apis[0].mine(3).await;
        ctx.sleep(Duration::from_secs(2)).await;
        assert_eq!(net.apis[1].tip().await.1, 9);
        assert_eq!(net.apis[0].tip().await.1, 8);
    });
}

#[test]
fn late_joiner_syncs_headers_first() {
    runner(5).start(|ctx| async move {
        let net = setup(&ctx, 3, 4, false).await;
        net.link(0, 1).await;
        ctx.sleep(Duration::from_secs(1)).await;
        for _ in 0..12 {
            net.apis[0].mine(3).await;
            ctx.sleep(Duration::from_millis(1000)).await;
        }
        wait_tips_equal(&ctx, &net.apis[..2], 20).await;
        assert_eq!(net.apis[2].tip().await.1, 0);
        net.link(2, 0).await;
        net.link(2, 1).await;
        let (_, h) = wait_tips_equal(&ctx, &net.apis, 120).await;
        assert_eq!(h, 12);
        let info = net.apis[2].call("getChainInfo", json!({})).await;
        assert_eq!(info["finalized_height"], json!(8));
    });
}

#[test]
fn withdrawal_becomes_final_with_proof() {
    runner(6).start(|ctx| async move {
        let net = setup(&ctx, 2, 3, true).await;
        ctx.sleep(Duration::from_secs(1)).await;
        let b64 = |t: &Tx| base64::engine::general_purpose::STANDARD.encode(t.encode());
        let msg = deposit_message(9, &pk(2), 5000);
        let sig = AttesterSig { pubkey: pk(50), sig: sign_plain(&seed(50), &msg) };
        let fund = Tx::signed(&seed(200), CHAIN, 0, 0, TxBody::BridgeUnlock { solana_nonce: 9, recipient: pk(2), amount_spl: 5000, job_order: None, sigs: vec![sig] });
        net.apis[0].call("sendTx", json!({"tx": b64(&fund)})).await;
        net.apis[0].mine(3).await;
        let lock = Tx::signed(&seed(2), CHAIN, 0, 10, TxBody::BridgeLock { solana_recipient: [9; 32], amount: 2_000_000 });
        net.apis[1].call("sendTx", json!({"tx": b64(&lock)})).await;
        ctx.sleep(Duration::from_millis(500)).await;
        net.apis[0].mine(3).await;
        let p = net.apis[0].call("getWithdrawalProof", json!({"nonce": 0})).await;
        assert_eq!(p["final"], json!(false));
        assert_eq!(p["withdrawal"]["amount_spl"], json!("2000"));
        for _ in 0..3 {
            ctx.sleep(Duration::from_millis(1000)).await;
            net.apis[0].mine(3).await;
        }
        wait_tips_equal(&ctx, &net.apis, 20).await;
        for api in &net.apis {
            let p = api.call("getWithdrawalProof", json!({"nonce": 0})).await;
            assert_eq!(p["final"], json!(true), "{p}");
            assert!(p["proof"].as_str().unwrap().len() > 10);
            let root_block = api.call("getHeader", json!({"hash": p["root_block_hash"]})).await;
            assert_eq!(root_block["header_fields"]["withdraw_root"], p["withdraw_root"]);
            let ev = api.call("getBridgeEvents", json!({})).await;
            assert_eq!(ev["withdrawal_count"], json!(1));
            assert_eq!(ev["withdrawals"][0]["final"], json!(true));
        }
        let esc = net.apis[1].call("getEscrow", json!({})).await;
        assert_eq!(esc["outside_escrow"], json!((5_000_000u64 - 2_000_000).to_string()));
    });
}

/// getWithdrawalProof `at_height`: a historical proof against an OLDER final header's
/// withdraw_root verifies with the light client's verifier (crates/pearl-verify-sbf), so
/// a light client need not sit at the node's final tip; bad heights are refused.
#[test]
fn historical_withdrawal_proof_at_an_older_final_header() {
    runner(7).start(|ctx| async move {
        let net = setup(&ctx, 1, 3, true).await;
        let api = &net.apis[0];
        let b64 = |t: &Tx| base64::engine::general_purpose::STANDARD.encode(t.encode());
        let msg = deposit_message(9, &pk(2), 5000);
        let sig = AttesterSig { pubkey: pk(50), sig: sign_plain(&seed(50), &msg) };
        let fund = Tx::signed(&seed(200), CHAIN, 0, 0, TxBody::BridgeUnlock { solana_nonce: 9, recipient: pk(2), amount_spl: 5000, job_order: None, sigs: vec![sig] });
        api.call("sendTx", json!({"tx": b64(&fund)})).await;
        api.mine(3).await;
        // withdrawal 0 in one block, withdrawal 1 in a later one
        for (n, amount) in [(0u64, 1_000_000u64), (1, 2_000_000)] {
            let lock = Tx::signed(&seed(2), CHAIN, n, 10, TxBody::BridgeLock { solana_recipient: [9; 32], amount });
            api.call("sendTx", json!({"tx": b64(&lock)})).await;
            api.mine(3).await;
            api.mine(3).await;
        }
        for _ in 0..5 {
            api.mine(3).await;
        }
        let w0 = api.call("getWithdrawalProof", json!({"nonce": 0})).await;
        let w1 = api.call("getWithdrawalProof", json!({"nonce": 1})).await;
        assert_eq!((w0["final"].clone(), w1["final"].clone()), (json!(true), json!(true)), "{w0} {w1}");
        let h0 = w0["withdrawal"]["height"].as_u64().unwrap();
        let h1 = w1["withdrawal"]["height"].as_u64().unwrap();
        assert!(h0 < h1);
        let final_h = api.call("getChainInfo", json!({})).await["finalized_height"].as_u64().unwrap();
        let sha: pearl_verify_sbf::mmr::Sha = |parts| {
            use sha2::Digest;
            let mut h = sha2::Sha256::new();
            for p in parts {
                h.update(p);
            }
            h.finalize().into()
        };
        let check = |p: &Value, root_hex: &str| -> bool {
            let entry = hex::decode(p["entry"].as_str().unwrap()).unwrap();
            let proof = base64::engine::general_purpose::STANDARD.decode(p["proof"].as_str().unwrap()).unwrap();
            let root: [u8; 32] = hex::decode(root_hex).unwrap().try_into().unwrap();
            let mut el = vec![0u8; 11 + entry.len()];
            let n = pearl_verify_sbf::mmr::append_element(&entry, &mut el);
            pearl_verify_sbf::mmr::verify_inclusion(sha, &proof, &el[..n], p["location"].as_u64().unwrap(), &root).is_ok()
        };
        let mut older_roots = 0;
        for at in h0..=final_h {
            let p = api.call("getWithdrawalProof", json!({"nonce": 0, "at_height": at})).await;
            let hdr = api.call("getHeader", json!({"height": at})).await;
            let root = hdr["header_fields"]["withdraw_root"].as_str().unwrap();
            assert_eq!(p["withdraw_root"].as_str().unwrap(), root, "root is the header's at {at}");
            assert_eq!(p["root_height"], json!(at));
            assert_eq!(p["root_block_hash"], hdr["hash"]);
            assert!(check(&p, root), "historical proof at {at} verifies");
            if root != w0["withdraw_root"].as_str().unwrap() {
                older_roots += 1;
                assert!(!check(&w0, root), "the tip proof does not verify against an older root");
            }
        }
        assert!(older_roots >= 2, "{older_roots}");
        // withdrawal 1 against its own block and later
        let p = api.call("getWithdrawalProof", json!({"nonce": 1, "at_height": h1})).await;
        assert!(check(&p, p["withdraw_root"].as_str().unwrap()));
        // refused: below the withdrawal's block, above final
        let try_call = |params: Value| {
            let api = api.clone();
            async move {
                let (tx, rx) = oneshot::channel();
                api.0.clone().send(ApiRequest::Call { method: "getWithdrawalProof".into(), params, reply: tx }).await.unwrap();
                rx.await.unwrap()
            }
        };
        let e = try_call(json!({"nonce": 1, "at_height": h1 - 1})).await.unwrap_err();
        assert!(e.message.contains("below the withdrawal"), "{}", e.message);
        let e = try_call(json!({"nonce": 0, "at_height": final_h + 1})).await.unwrap_err();
        assert!(e.message.contains("not final"), "{}", e.message);
    });
}

// ---------------------------------------------------------------- M2: data availability

fn b64e(b: &[u8]) -> String {
    base64::engine::general_purpose::STANDARD.encode(b)
}

fn unlock_tx(rnonce: u64, solana_nonce: u64, to: Pubkey, amount_spl: u64) -> Tx {
    let msg = deposit_message(solana_nonce, &to, amount_spl);
    let sig = AttesterSig { pubkey: pk(50), sig: sign_plain(&seed(50), &msg) };
    Tx::signed(&seed(200), CHAIN, rnonce, 0, TxBody::BridgeUnlock { solana_nonce, recipient: to, amount_spl, job_order: None, sigs: vec![sig] })
}

fn post_job(poster: u8, nonce: u64, data_uri: &str, p: u64, deadline: u64) -> Tx {
    let cfg = pearl_job::MiningConfig::localnet_default();
    let body = PostJob {
        m: 64,
        n: 64,
        k: cfg.k() as u32,
        config: cfg.to_bytes(),
        hash_a: [1; 32],
        hash_b: [2; 32],
        data_uri: data_uri.as_bytes().to_vec(),
        worker_pay: 1000,
        protocol_fee: p,
        deadline_height: deadline,
        max_blocks: 100,
        evaluators: vec![pk(60)],
        threshold: 1,
    };
    Tx::signed(&seed(poster), CHAIN, nonce, 10, TxBody::PostJob(body))
}

async fn has_blob(api: &Api, id: &str) -> bool {
    api.call("hasBlob", json!({"blob_id": id})).await["present"] == json!(true)
}

async fn wait_blob(ctx: &deterministic::Context, api: &Api, id: &str, secs: u64) -> bool {
    for _ in 0..secs * 10 {
        if has_blob(api, id).await {
            return true;
        }
        ctx.sleep(Duration::from_millis(100)).await;
    }
    false
}

#[test]
fn blobs_push_fetch_on_demand_and_template_availability() {
    runner(21).start(|ctx| async move {
        // line topology 0 - 1 - 2: a push reaches only direct peers
        let net = setup(&ctx, 3, 5, false).await;
        net.link(0, 1).await;
        net.link(1, 2).await;
        ctx.sleep(Duration::from_secs(1)).await;
        let data: Vec<u8> = (0..300_000u32).map(|i| (i * 31 % 251) as u8).collect();
        let put = net.apis[0].call("putBlob", json!({"data": b64e(&data)})).await;
        let id = put["blob_id"].as_str().unwrap().to_string();
        assert_eq!(id, hex::encode(blake3::hash(&data).as_bytes()));
        assert_eq!(put["uri"], json!(format!("blob:{id}")));
        assert_eq!(put["new"], json!(true));
        assert_eq!(net.apis[0].call("putBlob", json!({"data": b64e(&data)})).await["new"], json!(false));
        assert!(wait_blob(&ctx, &net.apis[1], &id, 10).await, "pushed to the direct peer");
        ctx.sleep(Duration::from_secs(2)).await;
        assert!(!has_blob(&net.apis[2], &id).await, "pushes are one hop");
        // an unreferenced blob is not fetched on a getBlob miss
        assert_eq!(net.apis[2].call("getBlob", json!({"blob_id": id})).await, Value::Null);
        assert_eq!(net.apis[2].call("hasBlob", json!({"blob_id": id})).await["fetching"], json!(false));
        // limits
        let e = net.apis[0].try_call("putBlob", json!({"data": b64e(&vec![1u8; (4 << 20) + 1])})).await.unwrap_err();
        assert!(e.contains("max_blob_bytes"), "{e}");
        assert!(net.apis[0].try_call("putBlob", json!({"data": ""})).await.unwrap_err().contains("empty"));

        // a job referencing the blob: node 2 fetches it by id once it sees the job
        net.apis[0].send(&unlock_tx(0, 1, pk(2), 100_000)).await.unwrap();
        net.apis[0].mine(9).await;
        let uri = format!("blob:{id}");
        // min protocol fee: P = 1 (alpha x P = 0) is refused before the mempool
        let e = net.apis[0].send(&post_job(2, 0, &uri, 1, 1000)).await.unwrap_err();
        assert!(e.contains("protocol fee"), "{e}");
        net.apis[0].send(&post_job(2, 0, &uri, 5_000_000, 1000)).await.unwrap();
        net.apis[0].mine(9).await;
        wait_tips_equal(&ctx, &net.apis, 20).await;
        assert!(wait_blob(&ctx, &net.apis[2], &id, 20).await, "node 2 fetched the job data from a peer");
        let got = net.apis[2].call("getBlob", json!({"uri": uri})).await;
        assert_eq!(got["data"], json!(b64e(&data)));
        let job1 = hex::encode(schema_job_id(&pk(2), 0));

        // job 2 has a bigger cap but its data exists nowhere: templates skip it
        let missing = format!("blob:{}", hex::encode(blake3::hash(b"never put").as_bytes()));
        net.apis[1].send(&post_job(2, 1, &missing, 50_000_000, 1000)).await.unwrap();
        ctx.sleep(Duration::from_secs(1)).await;
        net.apis[1].mine(9).await;
        wait_tips_equal(&ctx, &net.apis, 20).await;
        let job2 = hex::encode(schema_job_id(&pk(2), 1));
        let t = net.apis[1].call("getBlockTemplate", json!({"miner": bs58::encode(pk(9)).into_string()})).await;
        assert_eq!(t["job"]["job_id"], json!(job1), "auto skips the job without data");
        let e = net.apis[1].try_call("getBlockTemplate", json!({"miner": bs58::encode(pk(9)).into_string(), "job_id": job2})).await.unwrap_err();
        assert!(e.contains("not available"), "{e}");
        // mine a job-1 block on node 1; every node accepts it (consensus ignores blobs)
        let (t, r) = net.apis[1].mine_with(9, None).await;
        assert_eq!(t["job"]["job_id"], json!(job1));
        wait_tips_equal(&ctx, &net.apis, 20).await;
        // getBlock reports subsidy and fees (as the template predicted)
        let b = net.apis[2].call("getBlock", json!({"hash": r["block_hash"]})).await;
        assert_eq!(b["subsidy"], t["subsidy"]);
        assert_eq!(b["fees"], t["fees"]);
        assert!(b["subsidy"].as_str().unwrap().parse::<u64>().unwrap() > 0);
        let fb = net.apis[2].call("getBlock", json!({"height": 1})).await;
        assert_eq!(fb["subsidy"], json!("0"));
        // once the data shows up, job 2 (bigger cap) is offered
        net.apis[1].call("putBlob", json!({"data": b64e(b"never put")})).await;
        let t = net.apis[1].call("getBlockTemplate", json!({"miner": bs58::encode(pk(9)).into_string()})).await;
        assert_eq!(t["job"]["job_id"], json!(job2));
        // history by address over RPC, paginated
        let h = net.apis[2].call("getAccountTxs", json!({"account": bs58::encode(pk(2)).into_string(), "limit": 2})).await;
        assert_eq!(h["txs"].as_array().unwrap().len(), 2);
        assert_eq!(h["txs"][0]["kind"], json!("PostJob"));
        let next = h["next"].as_str().unwrap().to_string();
        let h2 = net.apis[2].call("getAccountTxs", json!({"account": bs58::encode(pk(2)).into_string(), "limit": 2, "before": next})).await;
        assert_eq!(h2["txs"].as_array().unwrap().len(), 1);
        assert_eq!(h2["txs"][0]["kind"], json!("BridgeUnlock"));
        assert_eq!(h2["txs"][0]["role"], json!("recipient"));
        assert_eq!(h2["next"], Value::Null);
        // getPeers in static mode
        let p = net.apis[1].call("getPeers", json!({})).await;
        assert_eq!(p["mode"], json!("static"));
        assert_eq!(p["count"], json!(2));
    });
}

fn schema_job_id(poster: &Pubkey, nonce: u64) -> Hash {
    state::schema::job_id_for_post(poster, nonce)
}

#[test]
fn blobs_pruned_after_job_closes_plus_grace() {
    runner(22).start(|ctx| async move {
        let net = setup(&ctx, 1, 3, true).await;
        let api = &net.apis[0];
        api.send(&unlock_tx(0, 1, pk(2), 100_000)).await.unwrap();
        api.mine(9).await;
        let loose = api.call("putBlob", json!({"data": b64e(b"nobody references me")})).await["blob_id"].as_str().unwrap().to_string();
        let data = api.call("putBlob", json!({"data": b64e(b"job data")})).await["blob_id"].as_str().unwrap().to_string();
        let deadline = 40;
        api.send(&post_job(2, 0, &format!("blob:{data}"), 5_000_000, deadline)).await.unwrap();
        api.mine(9).await; // height 2
        let job = hex::encode(schema_job_id(&pk(2), 0));
        // mine fallback blocks; prune runs every 10 blocks of tip movement
        let mut seen_loose_gone = 0;
        for _ in 2..deadline + 1 {
            api.mine(9).await;
            if seen_loose_gone == 0 && !has_blob(api, &loose).await {
                seen_loose_gone = api.tip().await.1;
            }
        }
        assert!(seen_loose_gone > 1 + BLOB_GRACE && seen_loose_gone <= 1 + BLOB_GRACE + 11, "unreferenced blob kept for the grace period, then pruned ({seen_loose_gone})");
        assert!(has_blob(api, &data).await, "open job keeps its data");
        // expire the job (deadline passed): kept for grace more blocks, then pruned
        api.send(&Tx::signed(&seed(2), CHAIN, 1, 10, TxBody::ExpireJob { job_id: hex::decode(&job).unwrap().try_into().unwrap() })).await.unwrap();
        api.mine(9).await;
        let closed = api.call("getJob", json!({"job_id": job})).await["closed_height"].as_u64().unwrap();
        assert!(closed > deadline);
        let mut gone_at = 0;
        for _ in 0..BLOB_GRACE + 25 {
            api.mine(9).await;
            let h = api.tip().await.1;
            if !has_blob(api, &data).await {
                gone_at = h;
                break;
            }
            assert!(h <= closed + BLOB_GRACE + 10, "blob not pruned by height {h}");
        }
        assert!(gone_at > closed + BLOB_GRACE, "pruned at {gone_at}, closed at {closed}");
        let s = api.call("getBlobStats", json!({})).await;
        assert_eq!(s["count"], json!(0));
    });
}

/// A node that was offline when the blob was pushed and catches up past finality (the job's
/// block connects and finalizes in one insert) still fetches the job data.
#[test]
fn late_joiner_fetches_job_blobs_after_catch_up() {
    runner(23).start(|ctx| async move {
        let net = setup(&ctx, 2, 3, false).await;
        let api = &net.apis[0];
        api.send(&unlock_tx(0, 1, pk(2), 100_000)).await.unwrap();
        api.mine(9).await;
        let id = api.call("putBlob", json!({"data": b64e(b"late joiner data")})).await["blob_id"].as_str().unwrap().to_string();
        api.send(&post_job(2, 0, &format!("blob:{id}"), 5_000_000, 1000)).await.unwrap();
        api.mine(9).await;
        for _ in 0..12 {
            api.mine(9).await;
        }
        assert!(!has_blob(&net.apis[1], &id).await);
        net.link(0, 1).await;
        wait_tips_equal(&ctx, &net.apis, 60).await;
        assert!(wait_blob(&ctx, &net.apis[1], &id, 30).await, "late joiner fetched the job data");
    });
}

/// R1 + R2 over the node RPC: every block's getBlock carries its coinbase receipt with a
/// tx_root path that the light client's verifier accepts (and tx paths); a payout lien
/// (SetRoute) on the miner's key splits its matured coinbase into withdrawal-log entries
/// from the miner to the lien's Solana recipient, provable like any BridgeLock; getRoute /
/// getAccount report it.
#[test]
fn coinbase_receipts_and_payout_lien_over_rpc() {
    runner(11).start(|ctx| async move {
        let net = setup(&ctx, 1, 3, true).await;
        let api = &net.apis[0];
        let b64 = |t: &Tx| base64::engine::general_purpose::STANDARD.encode(t.encode());
        let msg = deposit_message(9, &pk(3), 5000);
        let sig = AttesterSig { pubkey: pk(50), sig: sign_plain(&seed(50), &msg) };
        let fund = Tx::signed(&seed(200), CHAIN, 0, 0, TxBody::BridgeUnlock { solana_nonce: 9, recipient: pk(3), amount_spl: 5000, job_order: None, sigs: vec![sig] });
        api.call("sendTx", json!({"tx": b64(&fund)})).await;
        api.mine(4).await;
        // miner key pk(3) puts a 40% lien on its income, releasable by pk(70)
        let lender = [0x4c; 32];
        let set = Tx::signed(&seed(3), CHAIN, 0, 1_000_000, TxBody::SetRoute { solana_recipient: lender, route_bps: 4000, until_height: 1000, cap: u64::MAX, release_key: pk(70) });
        api.call("sendTx", json!({"tx": b64(&set)})).await;
        api.mine(3).await;
        let r = api.call("getRoute", json!({"account": bs58::encode(pk(3)).into_string()})).await;
        assert_eq!(r["active"], json!(true), "{r}");
        assert_eq!(r["route_bps"], json!(4000));
        assert_eq!(r["route_nonce"], json!(0));
        assert_eq!(r["release_key"], json!(bs58::encode(pk(70)).into_string()));
        // R8: the SetRoute's block, no clear yet
        let txs = api.call("getAccountTxs", json!({"account": bs58::encode(pk(3)).into_string(), "limit": 1})).await;
        let st = &txs["txs"][0];
        assert_eq!(st["kind"], json!("SetRoute"), "{txs}");
        assert_eq!(r["set_block"]["block_hash"], st["block_hash"], "{r}");
        assert_eq!(r["set_block"]["height"], st["height"]);
        assert_eq!(r["set_block"]["index"], st["index"]);
        assert_eq!(r["last_clear"], Value::Null);
        // a big-fee tx mined by pk(3), so its matured coinbase is worth splitting
        let t = Tx::signed(&seed(3), CHAIN, 1, 2_000_000, TxBody::Transfer { to: pk(5), amount: 1 });
        api.call("sendTx", json!({"tx": b64(&t)})).await;
        api.mine(3).await;
        for _ in 0..6 {
            api.mine(4).await;
        }
        let (_, tip) = api.tip().await;
        let mut receipts = 0;
        let mut cb_total = 0u128;
        for h in 1..=tip {
            let hdr = api.call("getHeader", json!({"height": h})).await;
            let blk = api.call("getBlock", json!({"height": h})).await;
            let hb: [u8; 256] = hex::decode(hdr["header"].as_str().unwrap()).unwrap().try_into().unwrap();
            let header = pearl_verify_sbf::header::Header(&hb);
            let cb = &blk["coinbase"];
            let rec = hex::decode(cb["record"].as_str().unwrap()).unwrap();
            let path = hex::decode(cb["path"].as_str().unwrap()).unwrap();
            let c = pearl_verify_sbf::chaintx::verify_coinbase(&header, &rec, &path).expect("receipt proves");
            assert_eq!(c.fees.to_string(), blk["fees"].as_str().unwrap());
            assert_eq!(cb["leaf_index"].as_u64().unwrap() as usize, blk["txs"].as_array().unwrap().len());
            for t in blk["txs"].as_array().unwrap() {
                let tx = base64::engine::general_purpose::STANDARD.decode(t["tx"].as_str().unwrap()).unwrap();
                let p = hex::decode(t["path"].as_str().unwrap()).unwrap();
                pearl_verify_sbf::chaintx::verify_tx_path(&pearl_verify_sbf::chaintx::tx_hash(&tx), &p, &header.tx_root()).unwrap();
            }
            if c.miner == pk(3) {
                cb_total += c.reward() as u128;
            }
            receipts += 1;
        }
        assert_eq!(receipts, tip);
        assert_eq!(cb_total, 3_000_000, "pk(3) mined the SetRoute (1e6) and the transfer (2e6) blocks");
        // both matured under the lien: 40% of 3e6 = 1.2e6 = 1200 SPL out in entries from pk(3)
        let r = api.call("getAccount", json!({"account": bs58::encode(pk(3)).into_string()})).await;
        assert_eq!(r["route"]["routed"], json!("1200000"), "{r}");
        assert_eq!(r["route"]["withdrawn"], json!("1200000"));
        let ev = api.call("getBridgeEvents", json!({})).await;
        let ws = ev["withdrawals"].as_array().unwrap();
        assert_eq!(ws.len(), 2, "{ev}");
        let total: u64 = ws.iter().map(|w| w["amount_spl"].as_str().unwrap().parse::<u64>().unwrap()).sum();
        assert_eq!(total, 1200);
        for w in ws {
            assert_eq!(w["from"], json!(bs58::encode(pk(3)).into_string()));
            assert_eq!(w["solana_recipient"], json!(bs58::encode(lender).into_string()));
        }
        let p = api.call("getWithdrawalProof", json!({"nonce": 0})).await;
        assert_eq!(p["withdrawal"]["from"], json!(bs58::encode(pk(3)).into_string()));
        // the release key clears it; later income is not split
        let cmsg = agent_types::tx::route_clear_message(CHAIN, &pk(3), 0);
        let clr = Tx::signed(&seed(3), CHAIN, 2, 10, TxBody::ClearRoute { account: pk(3), route_nonce: 0, sig: sign_plain(&seed(70), &cmsg) });
        api.send(&clr).await.unwrap();
        api.mine(4).await;
        let r = api.call("getRoute", json!({"account": bs58::encode(pk(3)).into_string()})).await;
        assert_eq!((r["active"].clone(), r["cleared"].clone()), (json!(false), json!(true)));
        // R7: the clear is in the account's history with role "lien"; R8 points at it
        let txs = api.call("getAccountTxs", json!({"account": bs58::encode(pk(3)).into_string(), "limit": 1})).await;
        let ct = &txs["txs"][0];
        assert_eq!((ct["kind"].clone(), ct["role"].clone()), (json!("ClearRoute"), json!("lien")), "{txs}");
        assert_eq!(r["last_clear"]["block_hash"], ct["block_hash"], "{r}");
        assert_eq!(r["last_clear"]["height"], ct["height"]);
        assert_eq!(r["last_clear"]["index"], ct["index"]);
        assert_eq!(r["last_clear"]["route_nonce"], json!(0));
        assert!(r["set_block"]["height"].as_u64().unwrap() < ct["height"].as_u64().unwrap());
    });
}

/// SPEC 4.5 over the node RPC: RegisterModel + model blobs (manifest pinned with its chunks,
/// uploaded with push false), an inference ticket, PostSegments, templates offering the
/// segment (kind "segment"), a mined segment block rolled up into getTicket / getModel.
#[test]
fn standing_jobs_over_rpc_and_model_blobs_pinned() {
    use agent_types::{MatrixSpec, RegisterModel, Segment};
    use pearl_job::standing;
    runner(31).start(|ctx| async move {
        let net = setup(&ctx, 2, 3, true).await;
        let api = &net.apis[0];
        api.send(&unlock_tx(0, 1, pk(2), 100_000)).await.unwrap();
        api.send(&unlock_tx(1, 2, pk(3), 100_000)).await.unwrap();
        api.send(&unlock_tx(2, 3, pk(4), 100_000)).await.unwrap();
        api.send(&unlock_tx(3, 4, agent_types::pool_address(), 1_000_000)).await.unwrap();
        api.mine(9).await;
        // model files: one chunk; the manifest lists it
        let chunk = vec![7u8; 1000];
        let chunk_id = hex::encode(blake3::hash(&chunk).as_bytes());
        let manifest = serde_json::to_vec(&json!({"format": "cadre-model-v1", "files": {"w/0.i8": {"size": 1000, "blake3": chunk_id, "blobs": [format!("blob:{chunk_id}")]}}})).unwrap();
        let manifest_id = hex::encode(blake3::hash(&manifest).as_bytes());
        let c = pearl_job::MiningConfig::localnet_default();
        let reg = RegisterModel {
            name: b"tiny".to_vec(),
            manifest_uri: format!("blob:{manifest_id}").into_bytes(),
            recipe: 1,
            rank: c.rank,
            rows_pattern: c.rows_pattern.to_bytes(),
            cols_pattern: c.cols_pattern.to_bytes(),
            matrices: vec![MatrixSpec { k: 2048, n: 8, hash_b: [0x61; 32] }],
        };
        api.send(&Tx::signed(&seed(2), CHAIN, 0, 10, TxBody::RegisterModel(reg))).await.unwrap();
        api.mine(9).await;
        let model = standing::model_id(&pk(2), b"tiny");
        let mhex = hex::encode(model);
        let m = api.call("getModel", json!({"model_id": mhex})).await;
        assert_eq!(m["name"], json!("tiny"));
        assert_eq!(m["macs_per_row"], json!((2048u64 * 8).to_string()));
        assert_eq!(m["matrices"][0]["job_key"], json!(hex::encode(standing::matrix_job_key(&model, 0, 2048, 8, &c))));
        assert_eq!(api.call("listModels", json!({})).await["total"], json!(1));
        // uploads after registration, manifest first; push false keeps them off node 1
        api.call("putBlob", json!({"data": b64e(&manifest), "push": false})).await;
        api.call("putBlob", json!({"data": b64e(&chunk), "push": false})).await;
        let loose = api.call("putBlob", json!({"data": b64e(b"loose"), "push": false})).await["blob_id"].as_str().unwrap().to_string();
        ctx.sleep(Duration::from_secs(2)).await;
        assert!(!has_blob(&net.apis[1], &chunk_id).await, "push false: not pushed");
        // ticket + segment
        let req = api.call("putBlob", json!({"data": b64e(b"{\"format\":\"cadre-request-v1\"}")})).await["uri"].as_str().unwrap().to_string();
        let a = api.call("putBlob", json!({"data": b64e(&standing::encode_activation_blob(4, 2048, &vec![1i8; 4 * 2048]).unwrap())})).await["uri"].as_str().unwrap().to_string();
        let ticket = PostJob {
            m: 3, n: 1, k: 1, config: standing::ticket_config(&model), hash_a: [5; 32], hash_b: pk(4),
            data_uri: req.into_bytes(), worker_pay: 1_000_000, protocol_fee: 2_000_000, deadline_height: 500, max_blocks: 2,
            evaluators: vec![pk(60)], threshold: 1,
        };
        api.send(&Tx::signed(&seed(3), CHAIN, 0, 10, TxBody::PostJob(ticket))).await.unwrap();
        api.mine(9).await;
        let tid = schema_job_id(&pk(3), 0);
        let seg = Segment { matrix: 0, m: 4, k: 2048, n: 8, max_blocks: 2, hash_a: [6; 32], hash_b: [0x61; 32], c_hash: [7; 32], a_uri: a.into_bytes() };
        let body = TxBody::PostSegments { ticket_id: tid, round: 0, model_id: model, rank: c.rank, rows_pattern: c.rows_pattern.to_bytes(), cols_pattern: c.cols_pattern.to_bytes(), segments: vec![seg] };
        api.send(&Tx::signed(&seed(4), CHAIN, 0, 10, body)).await.unwrap();
        api.mine(9).await;
        let t = api.call("getTicket", json!({"ticket_id": hex::encode(tid)})).await;
        assert_eq!(t["kind"], json!("ticket"));
        assert_eq!(t["mineable"], json!(false));
        assert_eq!(t["standing"]["server"], json!(bs58::encode(pk(4)).into_string()));
        assert_eq!(t["segment_list"].as_array().unwrap().len(), 1);
        let sid = hex::encode(standing::segment_id(&tid, 0, 0));
        // the template's auto choice is the segment
        let (tpl, _) = api.mine_with(9, None).await;
        assert_eq!(tpl["job"]["job_id"], json!(sid));
        assert_eq!(tpl["job"]["kind"], json!("segment"));
        assert_eq!(tpl["job"]["standing"]["model_id"], json!(mhex));
        assert_eq!(tpl["job"]["header_id"], json!(mhex));
        let t = api.call("getTicket", json!({"ticket_id": hex::encode(tid)})).await;
        assert_eq!(t["standing"]["blocks_mined"], json!(1));
        assert_eq!(t["segment_list"][0]["blocks_mined"], json!(1));
        assert_eq!(api.call("getModel", json!({"model_id": mhex})).await["blocks_mined"], json!(1));
        assert!(api.try_call("getTicket", json!({"ticket_id": sid})).await.is_err(), "a segment is not a ticket");
        // pinning: past the grace period the loose blob goes, the model's blobs stay
        for _ in 0..BLOB_GRACE + 15 {
            api.mine(9).await;
        }
        assert!(!has_blob(api, &loose).await, "unreferenced blob pruned");
        assert!(has_blob(api, &manifest_id).await, "manifest pinned");
        assert!(has_blob(api, &chunk_id).await, "model chunk pinned");
        // node 1 fetches a pinned chunk on demand once it holds the manifest
        assert!(wait_blob(&ctx, &net.apis[1], &manifest_id, 30).await, "manifest of a registered model is fetched");
        for _ in 0..12 {
            api.mine(9).await;
        }
        wait_tips_equal(&ctx, &net.apis, 20).await;
        let _ = net.apis[1].call("getBlob", json!({"blob_id": chunk_id})).await;
        assert!(wait_blob(&ctx, &net.apis[1], &chunk_id, 30).await, "chunk fetched on demand");
    });
}

/// Devnet 2026-10-20 regression: a model registered while its blobs were never uploaded
/// (`pearl model register` bailed at its inclusion wait, before the upload). Every node,
/// including the one the registrant talks to, wants the manifest and nobody serves it; the
/// fetch stays open through many prunes. Once the registrant uploads it (push false, a
/// devnet-sized 190 KB manifest) the peers get it over the blob resolver with no restart,
/// the uploading node cancels its own fetch, and a pinned chunk follows on demand.
#[test]
fn model_manifest_uploaded_long_after_registration_reaches_peers() {
    use agent_types::{MatrixSpec, RegisterModel};
    use pearl_job::standing;
    runner(32).start(|ctx| async move {
        let net = setup(&ctx, 3, 3, true).await;
        let api = &net.apis[0];
        api.send(&unlock_tx(0, 1, pk(2), 100_000)).await.unwrap();
        api.mine(9).await;
        let chunk = vec![5u8; 3 << 20];
        let chunk_id = hex::encode(blake3::hash(&chunk).as_bytes());
        // 144 files of 18 chunk ids each (only the first chunk exists): about 190 KB of JSON
        let mut files = serde_json::Map::new();
        for f in 0..144 {
            let blobs: Vec<String> = (0..18u32)
                .map(|c| if f == 0 && c == 0 { format!("blob:{chunk_id}") } else { format!("blob:{}", hex::encode(blake3::hash(&[f as u8, c as u8]).as_bytes())) })
                .collect();
            files.insert(format!("w/{f}.i8"), json!({"size": 1000, "blake3": chunk_id, "blobs": blobs}));
        }
        let manifest = serde_json::to_vec(&json!({"format": "cadre-model-v1", "files": files})).unwrap();
        assert!(manifest.len() > 180_000 && manifest.len() < (4 << 20), "{}", manifest.len());
        let manifest_id = hex::encode(blake3::hash(&manifest).as_bytes());
        let c = pearl_job::MiningConfig::localnet_default();
        let reg = RegisterModel {
            name: b"late".to_vec(),
            manifest_uri: format!("blob:{manifest_id}").into_bytes(),
            recipe: 1,
            rank: c.rank,
            rows_pattern: c.rows_pattern.to_bytes(),
            cols_pattern: c.cols_pattern.to_bytes(),
            matrices: vec![MatrixSpec { k: 2048, n: 8, hash_b: [0x62; 32] }],
        };
        api.send(&Tx::signed(&seed(2), CHAIN, 0, 10, TxBody::RegisterModel(reg))).await.unwrap();
        api.mine(9).await;
        let mhex = hex::encode(standing::model_id(&pk(2), b"late"));
        // no upload: many prune rounds and a long wait, every node fetching, nobody holding it
        for _ in 0..3 * BLOB_GRACE {
            api.mine(9).await;
        }
        wait_tips_equal(&ctx, &net.apis, 20).await;
        ctx.sleep(Duration::from_secs(600)).await;
        for a in &net.apis {
            let h = a.call("hasBlob", json!({"blob_id": manifest_id})).await;
            assert_eq!(h["present"], json!(false));
            assert_eq!(h["fetching"], json!(true), "a registered model's manifest is wanted");
            let m = a.call("getModel", json!({"model_id": mhex})).await;
            assert_eq!((m["manifest_present"].clone(), m["manifest_fetching"].clone()), (json!(false), json!(true)));
        }
        // the late upload on node 0 (what a rerun of `pearl model register` does)
        api.call("putBlob", json!({"data": b64e(&manifest), "push": false})).await;
        api.call("putBlob", json!({"data": b64e(&chunk), "push": false})).await;
        let h = api.call("hasBlob", json!({"blob_id": manifest_id})).await;
        assert_eq!((h["present"].clone(), h["fetching"].clone()), (json!(true), json!(false)));
        for (i, a) in net.apis.iter().enumerate().skip(1) {
            assert!(wait_blob(&ctx, a, &manifest_id, 30).await, "node {i} fetched the manifest after the late upload");
            let m = a.call("getModel", json!({"model_id": mhex})).await;
            assert_eq!((m["manifest_present"].clone(), m["manifest_fetching"].clone()), (json!(true), json!(false)));
        }
        // chunk pins come from the manifest at the next prune; then getBlob fetches on demand
        for _ in 0..12 {
            api.mine(9).await;
        }
        wait_tips_equal(&ctx, &net.apis, 20).await;
        let _ = net.apis[2].call("getBlob", json!({"blob_id": chunk_id})).await;
        assert!(wait_blob(&ctx, &net.apis[2], &chunk_id, 30).await, "pinned 3 MB chunk fetched on demand");
    });
}

/// The 8B upgrade (SPEC 4.5) over the network: a 640-matrix RegisterModel (25,815 bytes, over
/// the 16 KiB cap every other kind keeps) is accepted by node 1's RPC, gossiped to node 0,
/// mined there and the block carrying it reaches node 2; every node then serves the model.
/// A tx over the new 1024 limit is refused at the RPC.
#[test]
fn register_640_matrix_model_gossips_and_mines() {
    use agent_types::{MatrixSpec, RegisterModel};
    use pearl_job::standing;
    runner(33).start(|ctx| async move {
        let net = setup(&ctx, 3, 3, true).await;
        net.apis[0].send(&unlock_tx(0, 1, pk(2), 100_000)).await.unwrap();
        net.apis[0].mine(9).await;
        wait_tips_equal(&ctx, &net.apis, 20).await;
        let c = pearl_job::MiningConfig::localnet_default();
        let reg = |name: &[u8], count: usize| RegisterModel {
            name: name.to_vec(),
            manifest_uri: format!("blob:{}", hex::encode([0x44u8; 32])).into_bytes(),
            recipe: 1,
            rank: c.rank,
            rows_pattern: c.rows_pattern.to_bytes(),
            cols_pattern: c.cols_pattern.to_bytes(),
            matrices: (0..count).map(|i| MatrixSpec { k: 2048, n: if i < 608 { 4096 } else { 8016 }, hash_b: [(i % 251) as u8; 32] }).collect(),
        };
        let too_big = Tx::signed(&seed(2), CHAIN, 0, 10, TxBody::RegisterModel(reg(b"x", tx::MAX_MODEL_MATRICES + 1)));
        assert!(net.apis[1].send(&too_big).await.is_err(), "1025 matrices: refused");
        let t = Tx::signed(&seed(2), CHAIN, 0, 10, TxBody::RegisterModel(reg(b"llama-8b", 640)));
        assert_eq!(t.encode().len(), 25_815);
        net.apis[1].send(&t).await.unwrap();
        // gossiped to node 0's mempool
        let mut seen = false;
        for _ in 0..100 {
            if net.apis[0].call("getChainInfo", json!({})).await["mempool_size"].as_u64().unwrap() >= 1 {
                seen = true;
                break;
            }
            ctx.sleep(Duration::from_millis(100)).await;
        }
        assert!(seen, "the 640-matrix tx reached node 0 over p2p");
        net.apis[0].mine(9).await;
        wait_tips_equal(&ctx, &net.apis, 20).await;
        let mhex = hex::encode(standing::model_id(&pk(2), b"llama-8b"));
        for (i, a) in net.apis.iter().enumerate() {
            let m = a.call("getModel", json!({"model_id": mhex})).await;
            assert_eq!(m["matrix_count"], json!(640), "node {i}");
            assert_eq!(m["matrices"].as_array().unwrap().len(), 640, "node {i}");
        }
    });
}
