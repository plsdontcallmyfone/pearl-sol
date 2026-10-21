//! A FRESH node syncs the whole chain over p2p (the 2026-10-21 devnet stall: a new node stopped
//! at height 1-2 once the chain was longer than the orphan pool, 4096 blocks).
//!
//! The history runs on the public devnet's chain id, so both scheduled heights apply: the social
//! kinds from 3260 and the 1024-matrix RegisterModel from 4200. It carries every kind added
//! since: SetProfile, OperatorLink, Post, React, Follow, an AdminConfig on a post-activation key,
//! a 384-matrix RegisterModel before 4200 and a 640-matrix one (25,815 bytes, over the 16 KiB cap
//! other kinds keep) after it. Then a node with an empty data dir joins and must reach the tip
//! with the same finalized hash and the same social and model state.
//! Harness copied from network.rs.

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

const CHAIN: u32 = tx::DEVNET_CHAIN_ID;
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

fn unlock_tx(rnonce: u64, solana_nonce: u64, to: Pubkey, amount_spl: u64) -> Tx {
    let msg = deposit_message(solana_nonce, &to, amount_spl);
    let sig = AttesterSig { pubkey: pk(50), sig: sign_plain(&seed(50), &msg) };
    Tx::signed(&seed(200), CHAIN, rnonce, 0, TxBody::BridgeUnlock { solana_nonce, recipient: to, amount_spl, job_order: None, sigs: vec![sig] })
}

fn b58(k: &Pubkey) -> String {
    bs58::encode(k).into_string()
}

fn model(name: &[u8], count: usize) -> RegisterModel {
    let c = pearl_job::MiningConfig::localnet_default();
    RegisterModel {
        name: name.to_vec(),
        manifest_uri: format!("blob:{}", hex::encode([0x44u8; 32])).into_bytes(),
        recipe: 1,
        rank: c.rank,
        rows_pattern: c.rows_pattern.to_bytes(),
        cols_pattern: c.cols_pattern.to_bytes(),
        matrices: (0..count).map(|i| MatrixSpec { k: 2048, n: if i < 608 { 4096 } else { 8016 }, hash_b: [(i % 251) as u8; 32] }).collect(),
    }
}

/// Mine fallback blocks on node 0 until its tip is at `height`.
async fn mine_to(ctx: &deterministic::Context, api: &Api, height: u64) {
    let mut h = api.tip().await.1;
    while h < height {
        api.mine(9).await;
        h += 1;
        if h % 64 == 0 {
            ctx.sleep(Duration::from_millis(50)).await;
        }
    }
}

/// Mine one block on node 0 and check every tx in `txs` landed in it.
async fn mine_txs(api: &Api, txs: &[Tx]) {
    for t in txs {
        api.send(t).await.unwrap_or_else(|e| panic!("{:?}: {e}", t.body.kind()));
    }
    let r = api.mine(9).await;
    let b = api.call("getBlock", json!({"height": r["height"]})).await;
    assert_eq!(b["txs"].as_array().unwrap().len(), txs.len(), "{b}");
}

async fn wait_synced(ctx: &deterministic::Context, apis: &[Api], secs: u64) -> (String, u64) {
    for _ in 0..secs {
        let mut tips = Vec::new();
        for a in apis {
            tips.push(a.tip().await);
        }
        if tips.iter().all(|t| *t == tips[0]) {
            return tips[0].clone();
        }
        ctx.sleep(Duration::from_secs(1)).await;
    }
    let mut tips = Vec::new();
    for a in apis {
        tips.push(a.tip().await);
    }
    panic!("the fresh node did not sync: {tips:?}");
}

#[test]
fn fresh_node_syncs_devnet_history_past_both_upgrades() {
    use agent_types::social::{FollowEdge, Post, Reaction, SetProfile, DEVNET_SOCIAL_HEIGHT};
    use agent_types::tx::{DEVNET_MODEL_MATRICES_HEIGHT, MAX_MODEL_MATRICES_V1};
    runner(41).start(|ctx| async move {
        let k = 30;
        let net = setup(&ctx, 3, k, false).await;
        net.link(0, 1).await;
        ctx.sleep(Duration::from_secs(1)).await;
        let api = &net.apis[0];
        // funds for the senders (and the admin, pk(1))
        let funded = [1u8, 2, 3, 4];
        let unlocks: Vec<Tx> = funded.iter().enumerate().map(|(i, w)| unlock_tx(i as u64, 700 + i as u64, pk(*w), 1_000_000)).collect();
        mine_txs(api, &unlocks).await;
        // a RegisterModel at the original 384-matrix limit (15,354 bytes) before the upgrade
        let small = Tx::signed(&seed(4), CHAIN, 0, 10, TxBody::RegisterModel(model(b"llama-1b", MAX_MODEL_MATRICES_V1)));
        assert!(small.encode().len() > 15_000);
        mine_txs(api, &[small]).await;
        mine_to(&ctx, api, DEVNET_SOCIAL_HEIGHT - 2).await;
        // the social kinds are refused one block before activation
        let p0 = Tx::signed(&seed(2), CHAIN, 0, 1000, TxBody::SetProfile(SetProfile { handle: b"lamp_keeper".to_vec(), name: b"Lamp Keeper".to_vec(), flags: 1, ..Default::default() }));
        assert!(api.send(&p0).await.is_err(), "SetProfile below the social height");
        mine_to(&ctx, api, DEVNET_SOCIAL_HEIGHT - 1).await;
        // block DEVNET_SOCIAL_HEIGHT: profiles, an operator link, the admin sets a new key
        let prof3 = Tx::signed(&seed(3), CHAIN, 0, 1000, TxBody::SetProfile(SetProfile { handle: b"harbour_master".to_vec(), name: b"Harbour Master".to_vec(), ..Default::default() }));
        let link = Tx::signed(&seed(3), CHAIN, 1, 1000, TxBody::OperatorLink { agent: pk(2), add: true });
        let admin = Tx::signed(&seed(1), CHAIN, 0, 1000, TxBody::AdminConfig { key: state::config::keys::POST_INLINE_MAX, value: 2048u32.to_le_bytes().to_vec() });
        mine_txs(api, &[p0, prof3, link, admin]).await;
        assert_eq!(api.tip().await.1, DEVNET_SOCIAL_HEIGHT);
        // posts, a reply, a reaction with a tip, follows
        let post = agent_types::social::post_id(&pk(2), 1);
        mine_txs(api, &[Tx::signed(&seed(2), CHAIN, 1, 1000, TxBody::Post(Post { body: b"The fog came in.".to_vec(), thought_ref: b"req_abc".to_vec(), ..Default::default() }))]).await;
        let txs = [
            Tx::signed(&seed(3), CHAIN, 2, 1000, TxBody::Post(Post { kind: 1, parent: post, body: b"Saw it.".to_vec(), ..Default::default() })),
            Tx::signed(&seed(4), CHAIN, 1, 1000, TxBody::React { items: vec![Reaction { post_id: post, reaction: 4, tip: 7_000 }] }),
            Tx::signed(&seed(4), CHAIN, 2, 1000, TxBody::Follow { items: vec![FollowEdge { target: pk(2), on: true }, FollowEdge { target: pk(3), on: true }] }),
        ];
        mine_txs(api, &txs).await;
        mine_to(&ctx, api, DEVNET_MODEL_MATRICES_HEIGHT - 2).await;
        // a 640-matrix model is refused one block before the matrix upgrade ...
        let big = Tx::signed(&seed(2), CHAIN, 2, 1000, TxBody::RegisterModel(model(b"llama-8b", 640)));
        assert_eq!(big.encode().len(), 25_815);
        assert!(api.send(&big).await.is_err(), "640 matrices below the upgrade height");
        mine_to(&ctx, api, DEVNET_MODEL_MATRICES_HEIGHT - 1).await;
        // ... and lands in the first block at it
        mine_txs(api, &[big]).await;
        assert_eq!(api.tip().await.1, DEVNET_MODEL_MATRICES_HEIGHT);
        // past the old orphan-pool size (4096) by a few hundred blocks
        mine_to(&ctx, api, DEVNET_MODEL_MATRICES_HEIGHT + 150).await;
        let (tip, height) = wait_synced(&ctx, &net.apis[..2], 120).await;
        assert_eq!(height, DEVNET_MODEL_MATRICES_HEIGHT + 150);

        // a fresh node joins both seeds
        assert_eq!(net.apis[2].tip().await.1, 0);
        net.link(2, 0).await;
        net.link(2, 1).await;
        let (t2, h2) = wait_synced(&ctx, &net.apis, 1800).await;
        assert_eq!((t2, h2), (tip, height));
        let i0 = api.call("getChainInfo", json!({})).await;
        let i2 = net.apis[2].call("getChainInfo", json!({})).await;
        assert_eq!(i2["finalized_hash"], i0["finalized_hash"]);
        assert_eq!(i2["finalized_height"], json!(height - k as u64));
        assert_eq!(i2["syncing"], json!(false));
        // the fresh node's state agrees on what the new kinds did
        for (what, method, params) in [
            ("profile", "getProfile", json!({"account": b58(&pk(2))})),
            ("handle", "getHandle", json!({"handle": "harbour_master"})),
            ("post", "getPost", json!({"post_id": hex::encode(post)})),
            ("follows", "getFollowCounts", json!({"account": b58(&pk(3))})),
            ("params", "getParams", json!({})),
            ("model 8b", "getModel", json!({"model_id": hex::encode(pearl_job::standing::model_id(&pk(2), b"llama-8b"))})),
            ("model 1b", "getModel", json!({"model_id": hex::encode(pearl_job::standing::model_id(&pk(4), b"llama-1b"))})),
            ("balance", "getAccount", json!({"account": b58(&pk(2))})),
        ] {
            let a = api.call(method, params.clone()).await;
            let b = net.apis[2].call(method, params).await;
            assert!(!a.is_null(), "{what} on node 0");
            assert_eq!(a, b, "{what}");
        }
        let m = net.apis[2].call("getModel", json!({"model_id": hex::encode(pearl_job::standing::model_id(&pk(2), b"llama-8b"))})).await;
        assert_eq!(m["matrix_count"], json!(640));
        let p = net.apis[2].call("getParams", json!({})).await;
        assert_eq!(p["post_inline_max"], json!(2048));
        let _ = (&net.oracle, &net.keys);
    });
}
