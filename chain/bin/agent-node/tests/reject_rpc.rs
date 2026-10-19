//! RejectResult over the node RPC (SPEC 4.1, docs/RPC.md): a real node actor on the
//! deterministic runtime (harness copied from network.rs), getJob.reject_count,
//! getJobRejects, getRejectStats, getAccountTxs role "rejected".

#![allow(dead_code)]

use std::time::{Duration, UNIX_EPOCH};

use agent_node::node::{run, ActorConfig};
use agent_types::tx::{deposit_message, pubkey_from_seed, reject_message, result_message, sign_plain};
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
fn reject_result_over_rpc() {
    runner(21).start(|ctx| async move {
        let net = setup(&ctx, 1, 3, true).await;
        let api = &net.apis[0];
        let b58 = |k: &Pubkey| bs58::encode(k).into_string();
        // fund poster 2, deliverers 3 and 4
        for (i, who) in [2u8, 3, 4].iter().enumerate() {
            let msg = deposit_message(100 + i as u64, &pk(*who), 5000);
            let sig = AttesterSig { pubkey: pk(50), sig: sign_plain(&seed(50), &msg) };
            let t = Tx::signed(&seed(200), CHAIN, i as u64, 0, TxBody::BridgeUnlock { solana_nonce: 100 + i as u64, recipient: pk(*who), amount_spl: 5000, job_order: None, sigs: vec![sig] });
            api.send(&t).await.unwrap();
        }
        api.mine(9).await;
        let cfg = pearl_job::MiningConfig::localnet_default();
        let job = PostJob {
            m: 64, n: 64, k: cfg.k() as u32, config: cfg.to_bytes(), hash_a: [1; 32], hash_b: [2; 32],
            data_uri: b"ipfs://x".to_vec(), worker_pay: 1000, protocol_fee: 2_000_000, deadline_height: 500,
            max_blocks: 100, evaluators: vec![pk(60)], threshold: 1,
        };
        api.send(&Tx::signed(&seed(2), CHAIN, 0, 10, TxBody::PostJob(job))).await.unwrap();
        api.mine(9).await;
        let id = blake3_hash(&[&pk(2), &0u64.to_le_bytes()]);
        let idh = hex::encode(id);
        let bad = [0x66; 32];
        api.send(&Tx::signed(&seed(3), CHAIN, 0, 10, TxBody::SubmitResult { job_id: id, result_hash: bad, uri: vec![] })).await.unwrap();
        api.mine(9).await;
        let msg = reject_message(&id, &pk(3), &bad);
        let rj = Tx::signed(&seed(4), CHAIN, 0, 10, TxBody::RejectResult {
            job_id: id, deliverer: pk(3), result_hash: bad, sigs: vec![AttesterSig { pubkey: pk(60), sig: sign_plain(&seed(60), &msg) }],
        });
        api.send(&rj).await.unwrap();
        api.mine(9).await;
        let j = api.call("getJob", json!({"job_id": idh})).await;
        assert_eq!(j["status"], json!("open"), "{j}");
        assert_eq!(j["deliverer"], Value::Null);
        assert_eq!(j["reject_count"], json!(1));
        let r = api.call("getJobRejects", json!({"job_id": idh})).await;
        assert_eq!(r["count"], json!(1), "{r}");
        assert_eq!(r["rejects"][0]["deliverer"], json!(b58(&pk(3))));
        assert_eq!(r["rejects"][0]["result_hash"], json!(hex::encode(bad)));
        assert_eq!(r["rejects"][0]["protocol_fee"], json!("2000000"));
        assert_eq!(r["rejects"][0]["signers"], json!([b58(&pk(60))]));
        let s = api.call("getRejectStats", json!({"account": b58(&pk(3))})).await;
        assert_eq!((s["rejected"].clone(), s["rejected_fee"].clone()), (json!(1), json!("2000000")), "{s}");
        let s4 = api.call("getRejectStats", json!({"account": b58(&pk(4))})).await;
        assert_eq!(s4["rejected"], json!(0));
        // the rejected deliverer's history lists the reject with role "rejected"
        let txs = api.call("getAccountTxs", json!({"account": b58(&pk(3)), "limit": 1})).await;
        assert_eq!((txs["txs"][0]["kind"].clone(), txs["txs"][0]["role"].clone()), (json!("RejectResult"), json!("rejected")), "{txs}");
        // the sender's history lists it as sent
        let txs = api.call("getAccountTxs", json!({"account": b58(&pk(4)), "limit": 1})).await;
        assert_eq!(txs["txs"][0]["role"], json!("sender"));
        // rejected deliverer cannot redeliver; deliverer 4 can and is paid
        let again = Tx::signed(&seed(3), CHAIN, 1, 10, TxBody::SubmitResult { job_id: id, result_hash: [7; 32], uri: vec![] });
        let e = api.send(&again).await;
        api.mine(9).await;
        let j = api.call("getJob", json!({"job_id": idh})).await;
        assert!(e.is_err() || j["status"] == json!("open"), "a rejected deliverer's resubmission never lands: {j}");
        let good = [0x55; 32];
        api.send(&Tx::signed(&seed(4), CHAIN, 1, 10, TxBody::SubmitResult { job_id: id, result_hash: good, uri: vec![] })).await.unwrap();
        api.mine(9).await;
        let cmsg = result_message(&id, &pk(4), &good);
        api.send(&Tx::signed(&seed(4), CHAIN, 2, 10, TxBody::CompleteJob { job_id: id, sigs: vec![AttesterSig { pubkey: pk(60), sig: sign_plain(&seed(60), &cmsg) }] })).await.unwrap();
        api.mine(9).await;
        let j = api.call("getJob", json!({"job_id": idh})).await;
        assert_eq!((j["status"].clone(), j["deliverer"].clone()), (json!("completed"), json!(b58(&pk(4)))), "{j}");
        let _ = &net.oracle;
        let _ = &net.keys;
    });
}
