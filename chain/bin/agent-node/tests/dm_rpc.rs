//! DM and channel kinds over the node RPC (SPEC 4.10, docs/RPC.md): real node actors on the
//! deterministic runtime (harness copied from social_rpc.rs): sendTx admission (blocked
//! sender, short price, missing enc_key refused before the mempool), getDmGate, getDmParams,
//! escrowed pay released by a reply, getMessageEscrow, getMessage, getDmStats, getDmThread,
//! getConsent, channels (getChannel, listChannels, getChannelMember), getAccountTxs role
//! "message"; a reorg rolls DM state back (escrow, consent, channel, balances).

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

fn prof_dm(name: &str, policy: u8, price: u64, enc: Pubkey) -> TxBody {
    TxBody::SetProfile(SetProfile { name: name.as_bytes().to_vec(), flags: 1, dm_policy: policy, dm_price: price, enc_key: enc, ..Default::default() })
}

fn dm(to: u8, text: &str, pay: u64, reclaim_after: u32, reply_to: Hash) -> TxBody {
    TxBody::SendMessage(SendMessage { to: pk(to), body: text.as_bytes().to_vec(), pay, reclaim_after, reply_to, ..Default::default() })
}

#[test]
fn dms_over_rpc() {
    runner(41).start(|ctx| async move {
        let net = setup(&ctx, 1, 3, true).await;
        let api = &net.apis[0];
        for (i, who) in [2u8, 3, 4].iter().enumerate() {
            api.send(&unlock(i as u64, 600 + i as u64, *who, 5000)).await.unwrap();
        }
        api.mine(9).await;
        let p = api.call("getDmParams", json!({})).await;
        assert_eq!((p["msg_inline_max"].clone(), p["dm_reclaim_min"].clone(), p["active"].clone()), (json!(1024), json!(10), json!(true)), "{p}");
        // bob (3): priced cold DMs at 50000 with an enc_key; carol (4) blocks alice (2)
        api.send(&Tx::signed(&seed(3), CHAIN, 0, 1000, prof_dm("Bob", 1, 50_000, [0x33; 32]))).await.unwrap();
        api.send(&Tx::signed(&seed(4), CHAIN, 0, 1000, TxBody::SetConsent { items: vec![Consent { peer: pk(2), state: 2 }] })).await.unwrap();
        api.mine(9).await;
        let g = api.call("getDmGate", json!({"from": b58(&pk(2)), "to": b58(&pk(3))})).await;
        assert_eq!((g["can_send"].clone(), g["required_pay"].clone(), g["sealed_ok"].clone()), (json!(true), json!("50000"), json!(true)), "{g}");
        let g = api.call("getDmGate", json!({"from": b58(&pk(2)), "to": b58(&pk(4))})).await;
        assert_eq!((g["can_send"].clone(), g["consent"].clone()), (json!(false), json!("deny")), "the gate reads the recipient's consent for the sender: {g}");
        // admission: refused before the mempool
        let e = api.send(&Tx::signed(&seed(2), CHAIN, 0, 1000, dm(3, "cheap", 10, 0, [0; 32]))).await.unwrap_err();
        assert!(e.contains("price of 50000"), "{e}");
        let e = api.send(&Tx::signed(&seed(2), CHAIN, 0, 1000, dm(4, "hello?", 0, 0, [0; 32]))).await.unwrap_err();
        assert!(e.contains("blocked you"), "{e}");
        let e = api.send(&Tx::signed(&seed(2), CHAIN, 0, 10, dm(3, "fee too low", 50_000, 0, [0; 32]))).await.unwrap_err();
        assert!(e.contains("DM fee"), "{e}");
        let mut sealed = vec![1u8, 1];
        sealed.extend([7u8; 56]);
        sealed.extend(20u16.to_le_bytes());
        sealed.extend([9u8; 20]);
        let e = api.send(&Tx::signed(&seed(3), CHAIN, 1, 1000, TxBody::SendMessage(SendMessage { to: pk(2), mode: 1, body: sealed.clone(), ..Default::default() }))).await.unwrap_err();
        assert!(e.contains("no enc_key"), "{e}");
        assert_eq!(api.call("getMempool", json!({})).await["size"], json!(0));
        // an escrowed paid DM, then bob's reply releases it
        let id = agent_types::dm::message_id(&pk(2), 0);
        let b3 = api.balance(&pk(3)).await;
        api.send(&Tx::signed(&seed(2), CHAIN, 0, 1000, dm(3, "Will the fog lift?", 50_000, 30, [0; 32]))).await.unwrap();
        api.mine(9).await;
        let esc = api.call("getMessageEscrow", json!({"message_id": hex::encode(id)})).await;
        assert_eq!((esc["pay"].clone(), esc["reply_open"].clone(), esc["from"].clone()), (json!("50000"), json!(true), json!(b58(&pk(2)))), "{esc}");
        let m = api.call("getMessage", json!({"message_id": hex::encode(id)})).await;
        assert_eq!((m["body"].clone(), m["sealed"].clone(), m["pay"].clone(), m["to"].clone()), (json!("Will the fog lift?"), json!(false), json!("50000"), json!(b58(&pk(3)))), "{m}");
        assert!(m["escrow"].is_object() && m["tx_hash"].is_string());
        let g = api.call("getDmGate", json!({"from": b58(&pk(3)), "to": b58(&pk(2))})).await;
        assert_eq!(g["can_send"], json!(true));
        api.send(&Tx::signed(&seed(3), CHAIN, 1, 1000, dm(2, "At dawn.", 0, 0, id))).await.unwrap();
        api.mine(9).await;
        assert_eq!(api.call("getMessageEscrow", json!({"message_id": hex::encode(id)})).await, Value::Null);
        assert_eq!(api.balance(&pk(3)).await, b3 - 1000 + 50_000, "the reply fee out, the released pay in");
        let s = api.call("getDmStats", json!({"account": b58(&pk(3))})).await;
        assert_eq!((s["received"].clone(), s["answered"].clone(), s["pay_in"].clone()), (json!(1), json!(1), json!("50000")), "{s}");
        let t = api.call("getDmThread", json!({"a": b58(&pk(2)), "b": b58(&pk(3))})).await;
        assert!(t["last_a_to_b"].is_u64() && t["last_b_to_a"].is_u64(), "{t}");
        // a thread bob started: alice's follow-up is free now
        let g = api.call("getDmGate", json!({"from": b58(&pk(2)), "to": b58(&pk(3))})).await;
        assert_eq!((g["required_pay"].clone(), g["in_thread"].clone()), (json!("0"), json!(true)), "{g}");
        let c = api.call("getConsent", json!({"account": b58(&pk(4)), "peer": b58(&pk(2))})).await;
        assert_eq!(c["state"], json!("deny"));
        // a channel: create, join, list
        let ch = agent_types::dm::channel_id(&pk(2), 1);
        api.send(&Tx::signed(&seed(2), CHAIN, 1, 1000, TxBody::ChannelConfig(ChannelConfig { name: b"Market desk".to_vec(), about: b"bids".to_vec(), ..Default::default() }))).await.unwrap();
        api.mine(9).await;
        api.send(&Tx::signed(&seed(4), CHAIN, 1, 1000, TxBody::ChannelMember { channel: ch, action: 0, member: [0; 32], pay: 0 })).await.unwrap();
        api.mine(9).await;
        api.send(&Tx::signed(&seed(4), CHAIN, 2, 1000, TxBody::SendMessage(SendMessage { channel: ch, body: b"bid 40".to_vec(), ..Default::default() }))).await.unwrap();
        let e = api.send(&Tx::signed(&seed(3), CHAIN, 2, 1000, TxBody::SendMessage(SendMessage { channel: ch, body: b"not a member".to_vec(), ..Default::default() }))).await.unwrap_err();
        assert!(e.contains("only members"), "{e}");
        api.mine(9).await;
        let c = api.call("getChannel", json!({"channel": hex::encode(ch)})).await;
        assert_eq!((c["name"].clone(), c["members"].clone(), c["messages"].clone(), c["policy_name"].clone()), (json!("Market desk"), json!(2), json!(1), json!("open")), "{c}");
        let l = api.call("listChannels", json!({})).await;
        assert_eq!(l["total"], json!(1));
        let mm = api.call("getChannelMember", json!({"channel": hex::encode(ch), "account": b58(&pk(4))})).await;
        assert_eq!(mm["member"], json!(true));
        // history: bob's account lists alice's DM with role "message"
        let txs = api.call("getAccountTxs", json!({"account": b58(&pk(3)), "limit": 10})).await;
        let roles: Vec<String> = txs["txs"].as_array().unwrap().iter().map(|t| format!("{}:{}", t["kind"].as_str().unwrap(), t["role"].as_str().unwrap())).collect();
        assert!(roles.contains(&"SendMessage:message".to_string()), "{roles:?}");
        let _ = (&net.oracle, &net.keys);
    });
}

/// A reorg undoes DM state with the rest of the branch: the losing branch's escrow (and
/// the sender's locked pay), consent and channel are gone after node 0 adopts the heavier
/// branch, and the balance is whole again.
#[test]
fn dm_state_rolls_back_on_reorg() {
    runner(42).start(|ctx| async move {
        let net = setup(&ctx, 2, 10, true).await;
        ctx.sleep(Duration::from_secs(1)).await;
        net.apis[0].send(&unlock(0, 700, 2, 5000)).await.unwrap();
        net.apis[0].mine(3).await;
        wait_tips_equal(&ctx, &net.apis, 20).await;
        let before = net.apis[0].balance(&pk(2)).await;
        net.unlink(0, 1).await;
        let id = agent_types::dm::message_id(&pk(2), 0);
        let ch = agent_types::dm::channel_id(&pk(2), 2);
        net.apis[0].send(&Tx::signed(&seed(2), CHAIN, 0, 1000, dm(3, "only on the short branch", 40_000, 20, [0; 32]))).await.unwrap();
        net.apis[0].send(&Tx::signed(&seed(2), CHAIN, 1, 1000, TxBody::SetConsent { items: vec![Consent { peer: pk(5), state: 2 }] })).await.unwrap();
        net.apis[0].send(&Tx::signed(&seed(2), CHAIN, 2, 1000, TxBody::ChannelConfig(ChannelConfig { name: b"Ghost desk".to_vec(), ..Default::default() }))).await.unwrap();
        net.apis[0].mine(3).await;
        ctx.sleep(Duration::from_secs(1)).await;
        assert!(net.apis[0].call("getMessageEscrow", json!({"message_id": hex::encode(id)})).await.is_object());
        assert_eq!(net.apis[0].balance(&pk(2)).await, before - 3000 - 40_000);
        for _ in 0..3 {
            net.apis[1].mine(4).await;
            ctx.sleep(Duration::from_secs(1)).await;
        }
        net.link(0, 1).await;
        let (_, h) = wait_tips_equal(&ctx, &net.apis, 60).await;
        assert_eq!(h, 4);
        let a0 = &net.apis[0];
        assert_eq!(a0.call("getMessageEscrow", json!({"message_id": hex::encode(id)})).await, Value::Null, "escrow rolled back");
        assert_eq!(a0.call("getChannel", json!({"channel": hex::encode(ch)})).await, Value::Null, "channel rolled back");
        assert_eq!(a0.call("getConsent", json!({"account": b58(&pk(2)), "peer": b58(&pk(5))})).await["state"], Value::Null, "consent rolled back");
        assert_eq!(a0.call("getDmStats", json!({"account": b58(&pk(2))})).await["open_escrows"], json!(0));
        assert_eq!(a0.call("listChannels", json!({})).await["total"], json!(0));
        assert_eq!(a0.balance(&pk(2)).await, before, "the escrowed pay and fees are back");
    });
}
