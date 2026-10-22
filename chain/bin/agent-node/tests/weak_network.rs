//! Weak blocks and pre-confirmations across nodes (docs/SPEED.md 2): real node actors on
//! Commonware's deterministic runtime and simulated p2p network with latency, MockVerifier
//! proofs (weak = "mockweak" || pow_digest).

#![allow(dead_code)]

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
    c.target_block_time_ms = 60_000;
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
        NetConfig { max_size: 8 << 20, max_peers_per_set: std::num::NonZeroUsize::new(n + 1).unwrap(), disconnect_on_block: false, tracked_peer_sets: std::num::NonZeroUsize::new(3).unwrap() },
    );
    network.start();
    let signers: Vec<ed25519::PrivateKey> = (0..n + 1).map(|i| ed25519::PrivateKey::from_seed(100 + i as u64)).collect();
    let keys: Vec<ed25519::PublicKey> = signers.iter().map(|s| s.public_key()).collect();
    let mut manager = oracle.manager();
    let _ = manager.track(0, Set::try_from(keys.clone()).unwrap());
    let mut apis = Vec::new();
    for (i, key) in keys.iter().enumerate().take(n) {
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
        for a in 0..=n {
            for b in a + 1..=n {
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


fn b64(b: &[u8]) -> String {
    base64::engine::general_purpose::STANDARD.encode(b)
}

impl Api {
    /// Mine a weak block (proof meets only the weak target) on this node's template.
    async fn weak(&self, miner: u8) -> Value {
        let t = self.call("getBlockTemplate", json!({"miner": bs58::encode(pk(miner)).into_string(), "job_id": "fallback"})).await;
        assert_eq!(t["weak_shift"], json!(5));
        assert!(t["weak_target_bits"].is_u64());
        let mut h = Header::decode(&hex::decode(t["header"].as_str().unwrap()).unwrap()).unwrap();
        let proof = MockVerifier::weak_proof_for(&h);
        h.proof_hash = block::proof_hash(&proof);
        let r = self.call("submitWeakBlock", json!({"header": hex::encode(h.encode()), "proof": b64(&proof), "template_id": t["template_id"]})).await;
        assert_eq!(r["accepted"], json!(true), "{r}");
        r
    }

    async fn status(&self, tx: &Tx) -> Value {
        self.call("getTxStatus", json!({"tx_hash": hex::encode(tx.hash())})).await
    }
}

async fn wait_status(ctx: &deterministic::Context, api: &Api, tx: &Tx, want: &str, secs: u64) -> Value {
    for _ in 0..secs * 10 {
        let v = api.status(tx).await;
        if v["status"] == json!(want) {
            return v;
        }
        ctx.sleep(Duration::from_millis(100)).await;
    }
    panic!("status of {} never became {want}: {}", hex::encode(tx.hash()), api.status(tx).await);
}

#[test]
fn tx_preconfirmed_across_nodes_then_included_then_final() {
    runner(31).start(|ctx| async move {
        let net = setup(&ctx, 3, 3, true).await;
        ctx.sleep(Duration::from_secs(1)).await;
        // fund pk(2)
        let msg = deposit_message(1, &pk(2), 1000);
        let sig = AttesterSig { pubkey: pk(50), sig: sign_plain(&seed(50), &msg) };
        let fund = Tx::signed(&seed(200), CHAIN, 0, 0, TxBody::BridgeUnlock { solana_nonce: 1, recipient: pk(2), amount_spl: 1000, job_order: None, sigs: vec![sig] });
        net.apis[0].send(&fund).await.unwrap();
        net.apis[0].mine(9).await;
        wait_tips_equal(&ctx, &net.apis, 20).await;
        // a transfer sent to node 0
        let tx = Tx::signed(&seed(2), CHAIN, 0, 100, TxBody::Transfer { to: pk(3), amount: 5 });
        net.apis[0].send(&tx).await.unwrap();
        assert_eq!(net.apis[0].status(&tx).await["status"], json!("pending"));
        ctx.sleep(Duration::from_secs(1)).await;
        // node 1 has it through tx gossip; a miner on node 1 finds a weak block
        net.apis[1].weak(7).await;
        // node 2 sees it pre-confirmed through the gossiped weak block, and so does node 0
        let v = wait_status(&ctx, &net.apis[2], &tx, "preconfirmed", 10).await;
        assert_eq!(v["preconf"]["weak_count"], json!(1));
        assert_eq!(v["preconf"]["weak_miners"], json!(1));
        assert_eq!(v["preconf"]["confidence"], json!("low"));
        assert_eq!(v["final"], json!(false));
        let v0 = wait_status(&ctx, &net.apis[0], &tx, "preconfirmed", 10).await;
        assert_eq!(v0["preconf"]["weak_share"], json!(1.0));
        // getTx carries the same status and the body
        let g = net.apis[2].call("getTx", json!({"tx_hash": hex::encode(tx.hash())})).await;
        assert_eq!(g["status"], json!("preconfirmed"));
        assert_eq!(g["preconf"]["weak_count"], json!(1));
        // a second miner on node 2: medium confidence everywhere
        net.apis[2].weak(8).await;
        ctx.sleep(Duration::from_secs(1)).await;
        let v = net.apis[0].status(&tx).await;
        assert_eq!(v["preconf"]["weak_count"], json!(2));
        assert_eq!(v["preconf"]["weak_miners"], json!(2));
        assert_eq!(v["preconf"]["confidence"], json!("medium"));
        let wl = net.apis[1].call("getWeakBlocks", json!({})).await;
        assert_eq!(wl["count"], json!(2));
        // the preconfirmed tx cannot be replaced by a same-nonce, higher-fee tx
        let rep = Tx::signed(&seed(2), CHAIN, 0, 1000, TxBody::Transfer { to: pk(4), amount: 5 });
        let e = net.apis[0].send(&rep).await.unwrap_err();
        assert!(e.contains("preconfirmed"), "{e}");
        // a block includes it
        net.apis[1].mine(7).await;
        wait_tips_equal(&ctx, &net.apis, 20).await;
        for a in &net.apis {
            let v = a.status(&tx).await;
            assert_eq!(v["status"], json!("included"), "{v}");
            assert_eq!(v["confirmations"], json!(1));
        }
        // weak state of the old tip is pruned once the tip is 3 above it
        for _ in 0..3 {
            net.apis[0].mine(9).await;
        }
        wait_tips_equal(&ctx, &net.apis, 20).await;
        for a in &net.apis {
            let v = a.status(&tx).await;
            assert_eq!(v["status"], json!("final"), "{v}");
            assert_eq!(v["final"], json!(true));
            let g = a.call("getTx", json!({"tx_hash": hex::encode(tx.hash())})).await;
            assert_eq!(g["status"], json!("final"));
        }
    });
}

#[test]
fn tips_still_drive_catch_up_and_bad_weak_blocks_get_the_peer_blocked() {
    runner(32).start(|ctx| async move {
        let net = setup(&ctx, 3, 5, true).await;
        ctx.sleep(Duration::from_secs(1)).await;
        // node 2 misses blocks; the 40-byte tip announcements bring it back
        net.unlink(0, 2).await;
        net.unlink(1, 2).await;
        for _ in 0..3 {
            net.apis[0].mine(9).await;
        }
        net.link(0, 2).await;
        net.link(1, 2).await;
        let (_, h) = wait_tips_equal(&ctx, &net.apis, 30).await;
        assert_eq!(h, 3);
        // an outside peer (key 3) sends a well-formed weak block whose proof is garbage
        let evil = net.keys[3].clone();
        let ctl = net.oracle.control(evil.clone());
        let mut tips = ctl.register(net::CH_TIPS, net::quota()).await.unwrap();
        let t = net.apis[0].call("getBlockTemplate", json!({"miner": bs58::encode(pk(6)).into_string(), "job_id": "fallback"})).await;
        let mut hd = Header::decode(&hex::decode(t["header"].as_str().unwrap()).unwrap()).unwrap();
        let proof = b"garbage".to_vec();
        hd.proof_hash = block::proof_hash(&proof);
        let coinbase = hex::decode(t["coinbase"].as_str().unwrap()).unwrap();
        let cb = agent_types::Coinbase::decode(&coinbase).unwrap();
        let w = consensus_pow::WeakBlock { header: hd, proof, items: vec![cb.hash()], has_contracts: false, uncle_items: 0 };
        use commonware_p2p::Sender as _;
        let _ = tips.0.send(commonware_p2p::Recipients::One(net.keys[0].clone()), commonware_runtime::IoBuf::from(w.encode()), false);
        ctx.sleep(Duration::from_secs(2)).await;
        let blocked = net.oracle.blocked().await.unwrap();
        assert!(blocked.contains(&(net.keys[0].clone(), evil.clone())), "{blocked:?}");
        // an honest weak block from a node is not a reason to block anyone
        net.apis[1].weak(7).await;
        ctx.sleep(Duration::from_secs(2)).await;
        let blocked = net.oracle.blocked().await.unwrap();
        assert!(blocked.iter().all(|(_, b)| *b == evil), "{blocked:?}");
    });
}
