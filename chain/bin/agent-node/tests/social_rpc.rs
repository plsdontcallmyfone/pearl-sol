//! Social kinds over the node RPC (SPEC 4.6, docs/RPC.md): real node actors on the
//! deterministic runtime (harness copied from reject_rpc.rs): getProfile, getHandle,
//! listProfiles, getPost, getFollowCounts, isFollowing, getAccountTxs role "follow", sendTx's
//! social fee floor; a reorg rolls social state back; a profile's blob outlives the blob
//! grace period while the profile names it and is pruned after the profile moves on.

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



fn b64e(b: &[u8]) -> String {
    base64::engine::general_purpose::STANDARD.encode(b)
}
fn b58(k: &Pubkey) -> String {
    bs58::encode(k).into_string()
}
fn unlock(nonce: u64, sol_nonce: u64, who: u8, amount_spl: u64) -> Tx {
    let msg = deposit_message(sol_nonce, &pk(who), amount_spl);
    let sig = AttesterSig { pubkey: pk(50), sig: sign_plain(&seed(50), &msg) };
    Tx::signed(&seed(200), CHAIN, nonce, 0, TxBody::BridgeUnlock { solana_nonce: sol_nonce, recipient: pk(who), amount_spl, job_order: None, sigs: vec![sig] })
}
fn prof(handle: &str, name: &str, uri: &[u8], hash: Hash) -> TxBody {
    TxBody::SetProfile(SetProfile { handle: handle.as_bytes().to_vec(), name: name.as_bytes().to_vec(), profile_uri: uri.to_vec(), profile_hash: hash, flags: 1, ..Default::default() })
}

#[test]
fn social_over_rpc() {
    runner(31).start(|ctx| async move {
        let net = setup(&ctx, 1, 3, true).await;
        let api = &net.apis[0];
        for (i, who) in [2u8, 3, 4].iter().enumerate() {
            api.send(&unlock(i as u64, 300 + i as u64, *who, 5000)).await.unwrap();
        }
        api.mine(9).await;
        // the profile blob first (as cadre-api does), then SetProfile naming it
        let pb = br#"{"format":"cadre-profile-v1","bio":"keeps the lamp lit","appearance":"ink on his sleeves"}"#;
        let put = api.call("putBlob", json!({"data": b64e(pb)})).await;
        let id: Hash = hex::decode(put["blob_id"].as_str().unwrap()).unwrap().try_into().unwrap();
        assert_eq!(id, blake3_hash(&[pb]));
        // below the social fee floor (social_min_fee 1000 > this genesis' min_fee 10): refused up front
        let cheap = Tx::signed(&seed(2), CHAIN, 0, 10, prof("lamp_keeper", "Lamp Keeper", format!("blob:{}", hex::encode(id)).as_bytes(), id));
        let e = api.send(&cheap).await.unwrap_err();
        assert!(e.contains("social fee"), "{e}");
        api.send(&Tx::signed(&seed(2), CHAIN, 0, 1000, prof("lamp_keeper", "Lamp Keeper", format!("blob:{}", hex::encode(id)).as_bytes(), id))).await.unwrap();
        api.send(&Tx::signed(&seed(3), CHAIN, 0, 1000, prof("harbour_master", "Harbour Master", b"", [0; 32]))).await.unwrap();
        api.mine(9).await;
        let p = api.call("getProfile", json!({"account": b58(&pk(2))})).await;
        assert_eq!((p["handle"].clone(), p["name"].clone(), p["is_agent"].clone(), p["has_profile"].clone()), (json!("lamp_keeper"), json!("Lamp Keeper"), json!(true), json!(true)), "{p}");
        assert_eq!(p["profile_hash"], json!(hex::encode(id)));
        let h = api.call("getHandle", json!({"handle": "@Lamp_Keeper"})).await;
        assert_eq!(h["account"], json!(b58(&pk(2))));
        assert_eq!(api.call("getHandle", json!({"handle": "nobody"})).await, Value::Null);
        let l = api.call("listProfiles", json!({"limit": 10})).await;
        assert_eq!(l["total"], json!(2));
        let mut hs: Vec<String> = l["profiles"].as_array().unwrap().iter().map(|x| x["handle"].as_str().unwrap().to_string()).collect();
        hs.sort();
        assert_eq!(hs, vec!["harbour_master", "lamp_keeper"]);
        assert_eq!(api.call("getProfile", json!({"account": b58(&pk(4))})).await, Value::Null);
        // a post, a reply, a tip, a follow
        let p0 = agent_types::social::post_id(&pk(2), 1);
        api.send(&Tx::signed(&seed(2), CHAIN, 1, 1000, TxBody::Post(Post { body: b"The fog came in.".to_vec(), thought_ref: b"req_abc".to_vec(), ..Default::default() }))).await.unwrap();
        api.mine(9).await;
        api.send(&Tx::signed(&seed(3), CHAIN, 1, 1000, TxBody::Post(Post { kind: 1, parent: p0, body: b"Saw it.".to_vec(), ..Default::default() }))).await.unwrap();
        api.send(&Tx::signed(&seed(4), CHAIN, 0, 1000, TxBody::React { items: vec![Reaction { post_id: p0, reaction: 4, tip: 7_000 }] })).await.unwrap();
        api.send(&Tx::signed(&seed(4), CHAIN, 1, 1000, TxBody::Follow { items: vec![FollowEdge { target: pk(2), on: true }, FollowEdge { target: pk(3), on: true }] })).await.unwrap();
        api.mine(9).await;
        let g = api.call("getPost", json!({"post_id": hex::encode(p0)})).await;
        assert_eq!((g["author"].clone(), g["body"].clone(), g["thought_ref"].clone(), g["kind_name"].clone()), (json!(b58(&pk(2))), json!("The fog came in."), json!("req_abc"), json!("post")), "{g}");
        assert!(g["tx_hash"].is_string() && g["timestamp_ms"].is_u64());
        let p = api.call("getProfile", json!({"account": b58(&pk(2))})).await;
        assert_eq!((p["stats"]["followers"].clone(), p["stats"]["tips_received"].clone(), p["stats"]["reactions_received"].clone()), (json!(1), json!("7000"), json!(1)), "{p}");
        // an account with only social activity: has_profile false
        let p4 = api.call("getProfile", json!({"account": b58(&pk(4))})).await;
        assert_eq!((p4["has_profile"].clone(), p4["stats"]["following"].clone()), (json!(false), json!(2)));
        let fc = api.call("getFollowCounts", json!({"account": b58(&pk(3))})).await;
        assert_eq!(fc["followers"], json!(1));
        let f = api.call("isFollowing", json!({"follower": b58(&pk(4)), "target": b58(&pk(2))})).await;
        assert_eq!(f["following"], json!(true));
        assert!(f["since_height"].is_u64());
        let txs = api.call("getAccountTxs", json!({"account": b58(&pk(3)), "limit": 5})).await;
        let roles: Vec<String> = txs["txs"].as_array().unwrap().iter().map(|t| format!("{}:{}", t["kind"].as_str().unwrap(), t["role"].as_str().unwrap())).collect();
        assert!(roles.contains(&"Follow:follow".to_string()), "{roles:?}");
        let pr = api.call("getParams", json!({})).await;
        assert_eq!((pr["social_min_fee"].clone(), pr["post_inline_max"].clone(), pr["max_balance_rows"].clone()), (json!("1000"), json!(1024), json!(4096)));
        assert_eq!(pr["price_mlamports_per_gflop"], Value::Null);
        assert_eq!(pr["keys"]["social_byte_fee"], json!(20));
        let _ = (&net.oracle, &net.keys);
    });
}

/// A reorg undoes social state with the rest of the branch: the losing branch's profile,
/// handle, post and follow are gone after node 0 adopts the heavier branch.
#[test]
fn social_state_rolls_back_on_reorg() {
    runner(32).start(|ctx| async move {
        let net = setup(&ctx, 2, 10, true).await;
        ctx.sleep(Duration::from_secs(1)).await;
        net.apis[0].send(&unlock(0, 400, 2, 5000)).await.unwrap();
        net.apis[0].mine(3).await;
        wait_tips_equal(&ctx, &net.apis, 20).await;
        net.unlink(0, 1).await;
        // node 0's branch: a profile, a post and a follow
        net.apis[0].send(&Tx::signed(&seed(2), CHAIN, 0, 1000, prof("ghost_town", "Ghost", b"", [0; 32]))).await.unwrap();
        net.apis[0].send(&Tx::signed(&seed(2), CHAIN, 1, 1000, TxBody::Post(Post { body: b"only on the short branch".to_vec(), ..Default::default() }))).await.unwrap();
        net.apis[0].send(&Tx::signed(&seed(2), CHAIN, 2, 1000, TxBody::Follow { items: vec![FollowEdge { target: pk(3), on: true }] })).await.unwrap();
        net.apis[0].mine(3).await;
        ctx.sleep(Duration::from_secs(1)).await;
        assert_eq!(net.apis[0].call("getHandle", json!({"handle": "ghost_town"})).await["account"], json!(b58(&pk(2))));
        // node 1 mines a heavier branch without them
        for _ in 0..3 {
            net.apis[1].mine(4).await;
            ctx.sleep(Duration::from_secs(1)).await;
        }
        net.link(0, 1).await;
        let (_, h) = wait_tips_equal(&ctx, &net.apis, 60).await;
        assert_eq!(h, 4);
        let a0 = &net.apis[0];
        assert_eq!(a0.call("getHandle", json!({"handle": "ghost_town"})).await, Value::Null, "handle rolled back");
        assert_eq!(a0.call("getProfile", json!({"account": b58(&pk(2))})).await, Value::Null, "profile and stats rolled back");
        assert_eq!(a0.call("getFollowCounts", json!({"account": b58(&pk(3))})).await["followers"], json!(0));
        assert_eq!(a0.call("getPost", json!({"post_id": hex::encode(agent_types::social::post_id(&pk(2), 1))})).await, Value::Null);
        assert_eq!(a0.call("listProfiles", json!({})).await["total"], json!(0));
    });
}

/// SPEC 4.6 blob retention: a profile's blob is kept past the grace period while the profile
/// names it; once the profile moves to another blob the old one is pruned like any loose blob.
#[test]
fn profile_blob_kept_while_referenced_then_pruned() {
    runner(33).start(|ctx| async move {
        let net = setup(&ctx, 1, 3, true).await;
        let api = &net.apis[0];
        api.send(&unlock(0, 500, 2, 5000)).await.unwrap();
        api.mine(9).await;
        let put = |d: &'static [u8]| async move { api.call("putBlob", json!({"data": b64e(d)})).await["blob_id"].as_str().unwrap().to_string() };
        let old = put(br#"{"format":"cadre-profile-v1","bio":"first"}"#).await;
        let loose = put(b"nobody names me").await;
        let oid: Hash = hex::decode(&old).unwrap().try_into().unwrap();
        api.send(&Tx::signed(&seed(2), CHAIN, 0, 1000, prof("keeper", "Keeper", format!("blob:{old}").as_bytes(), oid))).await.unwrap();
        api.mine(9).await;
        let has = |id: String| async move { api.call("hasBlob", json!({"blob_id": id})).await["present"] == json!(true) };
        for _ in 0..BLOB_GRACE + 15 {
            api.mine(9).await;
        }
        assert!(!has(loose.clone()).await, "a loose blob is pruned after the grace period");
        assert!(has(old.clone()).await, "the profile's blob is kept while the profile names it");
        // move the profile to a new blob: the new one is kept, the old one is released
        let new = put(br#"{"format":"cadre-profile-v1","bio":"second"}"#).await;
        let nid: Hash = hex::decode(&new).unwrap().try_into().unwrap();
        api.send(&Tx::signed(&seed(2), CHAIN, 1, 1000, prof("keeper", "Keeper", format!("blob:{new}").as_bytes(), nid))).await.unwrap();
        api.mine(9).await;
        for _ in 0..12 {
            api.mine(9).await;
        }
        assert!(!has(old.clone()).await, "the old profile blob (stored long ago) is pruned once nothing names it");
        for _ in 0..BLOB_GRACE + 15 {
            api.mine(9).await;
        }
        assert!(has(new.clone()).await, "the current profile blob stays");
    });
}
