//! A node syncs from an honest peer while another peer floods it with junk headers
//! (C-U1, C-U2).
//!
//! The flooding peer announces tips it made up and serves a header for each: a job header on
//! an unknown random parent, 300 of them at the node's tip + 1 and 300 at tip + 1 .. tip + 300.
//! Each costs nothing to make, its work cannot be checked, and no peer can serve its body or
//! its parent. Before the fix the junk filled the body window for good (the node never asked
//! for an honest body again and stayed at its height), and the node kept asking every peer for
//! each junk parent header forever. Now the node reaches the honest tip, and the fetches for
//! the junk parents stop once they expire.
//! Harness copied from network.rs.

#![allow(dead_code)]

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, UNIX_EPOCH};

use agent_node::node::{run, ActorConfig};
use agent_types::block::encode_header_with_proof;
use agent_types::tx::pubkey_from_seed;
use agent_types::*;
use base64::Engine as _;
use bytes::Bytes;
use commonware_cryptography::{ed25519, Signer as _};
use commonware_p2p::simulated::{Config as NetConfig, Link, Network, Oracle};
use commonware_p2p::{Manager as _, Recipients, Sender as _};
use commonware_runtime::{deterministic, Clock, IoBuf, Runner as _, Spawner as _, Supervisor as _};
use commonware_utils::ordered::Set;
use consensus_pow::{Chain, Genesis, MockVerifier};
use futures::channel::{mpsc, oneshot};
use futures::SinkExt;
use rpc::{ApiRequest, Core};
use serde_json::{json, Value};
use state::config::Config;
use sync::{Kind, SyncEvent};

const CHAIN: u32 = 7;
const T0: u64 = 1_760_000_000_000;

fn pk(i: u8) -> Pubkey {
    pubkey_from_seed(&[i; 32])
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
}

const LINK: Link = Link { latency: Duration::from_millis(20), jitter: Duration::from_millis(5), success_rate: commonware_utils::probability!(1.0) };

type Ctl = commonware_p2p::simulated::Control<ed25519::PublicKey, deterministic::Context>;

struct Net {
    oracle: Oracle<ed25519::PublicKey, deterministic::Context>,
    keys: Vec<ed25519::PublicKey>,
    /// Nodes 0 (honest, holds the chain) and 1 (the node under test).
    apis: Vec<Api>,
    /// Control of the last key: the flooding peer, a bare p2p participant.
    flooder: Ctl,
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

/// Two real nodes and one more key on the network with no node behind it.
async fn setup(ctx: &deterministic::Context, k: u32) -> Net {
    let n = 3;
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
    for (i, key) in keys.iter().enumerate().take(2) {
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
        let store = blobs::BlobStore::open(ctx.child("blobs").with_attribute("node", i), &format!("{prefix}-blobs"), blobs::BlobConfig::default(), 0).await.unwrap();
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
    let flooder = oracle.control(keys[2].clone());
    Net { oracle, keys, apis, flooder }
}

fn runner(seed: u64) -> deterministic::Runner {
    deterministic::Runner::new(
        deterministic::Config::new()
            .with_seed(seed)
            .with_start_time(UNIX_EPOCH + Duration::from_millis(T0 + 1000))
            .with_timeout(Some(Duration::from_secs(3600))),
    )
}

async fn wait_synced(ctx: &deterministic::Context, apis: &[Api], secs: u64, what: &str) -> (String, u64) {
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
    panic!("{what}: {tips:?}");
}

/// A job header on an unknown random parent, with the bytes a peer serves for it.
fn junk_header(i: u64, height: u64) -> (Hash, Hash, Vec<u8>) {
    let mut proof = vec![0u8; 16];
    proof[..8].copy_from_slice(&i.to_le_bytes());
    let mut prev = [0xee; 32];
    prev[..8].copy_from_slice(&i.to_le_bytes());
    prev[8..16].copy_from_slice(&i.wrapping_mul(0x9e37_79b9_7f4a_7c15).to_le_bytes());
    let header = Header {
        version: 1,
        height,
        prev_hash: prev,
        timestamp_ms: T0 + 1000 * height,
        target_bits: 0x207fffff,
        miner: [7; 32],
        job_id: [0x42; 32],
        proof_hash: block::proof_hash(&proof),
        ..Default::default()
    };
    (header.hash(), prev, encode_header_with_proof(&header, &proof))
}

/// What the flooding peer saw of the node's header requests.
#[derive(Default)]
struct Seen {
    /// Junk headers it served (by hash, once each).
    served: Mutex<std::collections::HashSet<Hash>>,
    /// Requests for the junk headers' made-up parents (nobody has them).
    parent_asks: AtomicU64,
}

#[test]
fn node_syncs_from_an_honest_peer_under_a_junk_header_flood() {
    runner(77).start(|ctx| async move {
        let net = setup(&ctx, 30).await;
        let (honest, node) = (&net.apis[0], &net.apis[1]);
        // a shared start of 5 blocks, then the honest peer goes on alone to height 65
        net.link(0, 1).await;
        ctx.sleep(Duration::from_secs(1)).await;
        for _ in 0..5 {
            honest.mine(9).await;
        }
        let (_, h) = wait_synced(&ctx, &net.apis, 30, "the shared start did not sync").await;
        assert_eq!(h, 5);
        net.unlink(0, 1).await;
        for i in 0..60 {
            honest.mine(9).await;
            if i % 16 == 0 {
                ctx.sleep(Duration::from_millis(50)).await;
            }
        }
        let honest_tip = honest.tip().await;
        assert_eq!(honest_tip.1, 65);
        assert_eq!(node.tip().await.1, 5);

        // the flooding peer: a header resolver that serves its junk, and a tips sender
        let junk: Vec<(Hash, Hash, Vec<u8>)> = (0..600u64).map(|i| junk_header(i, if i < 300 { 6 } else { 6 + (i - 300) })).collect();
        let payloads: Arc<HashMap<Hash, Vec<u8>>> = Arc::new(junk.iter().map(|(h, _, p)| (*h, p.clone())).collect());
        let parents: Arc<std::collections::HashSet<Hash>> = Arc::new(junk.iter().map(|(_, p, _)| *p).collect());
        let seen = Arc::new(Seen::default());
        let q = net::quota();
        let ctl = net.flooder.clone();
        let (mut tips_tx, _tips_rx) = ctl.clone().register(net::CH_TIPS, q).await.unwrap();
        let headers_ch = ctl.clone().register(net::CH_HEADERS, q).await.unwrap();
        let bodies_ch = ctl.clone().register(net::CH_BODIES, q).await.unwrap();
        let (ev_tx, mut ev_rx) = commonware_utils::channel::mpsc::unbounded_channel::<SyncEvent>();
        let _headers = sync::start(ctx.child("flooder_headers"), Kind::Header, net.keys[2].clone(), net.oracle.manager(), ctl.clone(), headers_ch, ev_tx.clone());
        // it answers body requests too, always with "do not have it"
        let _bodies = sync::start(ctx.child("flooder_bodies"), Kind::Body, net.keys[2].clone(), net.oracle.manager(), ctl.clone(), bodies_ch, ev_tx);
        {
            let (payloads, parents, seen) = (payloads.clone(), parents.clone(), seen.clone());
            ctx.child("flooder").spawn(move |_| async move {
                while let Some(ev) = ev_rx.recv().await {
                    if let SyncEvent::Serve { kind: Kind::Header, key, reply } = ev {
                        if let Some(p) = payloads.get(&key) {
                            seen.served.lock().unwrap().insert(key);
                            let _ = reply.send(Bytes::from(p.clone()));
                        } else if parents.contains(&key) {
                            seen.parent_asks.fetch_add(1, Ordering::Relaxed);
                        }
                    }
                }
            });
        }
        net.link(1, 2).await;
        ctx.sleep(Duration::from_millis(500)).await;
        // 8 made-up tips every 100 ms: each is fetched, served and stored within the interval
        for batch in junk.chunks(8) {
            for (hash, _, _) in batch {
                let mut m = hash.to_vec();
                m.extend_from_slice(&1_000u64.to_le_bytes());
                let _ = tips_tx.send(Recipients::One(net.keys[1].clone()), IoBuf::from(m), false);
            }
            ctx.sleep(Duration::from_millis(100)).await;
        }
        ctx.sleep(Duration::from_secs(2)).await;
        let served = seen.served.lock().unwrap().len();
        assert_eq!(served, 600, "the node did not take the whole flood: the test would prove nothing");
        assert!(seen.parent_asks.load(Ordering::Relaxed) > 0, "the node asks for the junk parents at first");
        assert_eq!(node.tip().await.1, 5);

        // C-U1: the honest peer comes back while the flooder stays connected; the node must
        // ask for the honest bodies and reach the honest tip
        net.link(0, 1).await;
        let synced = wait_synced(&ctx, &net.apis, 60, "the node did not sync from the honest peer under the flood").await;
        assert_eq!(synced, honest_tip);

        // it keeps following new blocks
        honest.mine(9).await;
        let after = wait_synced(&ctx, &net.apis, 20, "the node stopped following the tip").await;
        assert_eq!(after.1, 66);

        // C-U2: the fetches for the 600 made-up parents expire. Well past the timeout the
        // node no longer asks anyone for them.
        ctx.sleep(Duration::from_secs(40)).await;
        let before = seen.parent_asks.load(Ordering::Relaxed);
        ctx.sleep(Duration::from_secs(20)).await;
        let late = seen.parent_asks.load(Ordering::Relaxed) - before;
        assert_eq!(late, 0, "the node still asks for junk parent headers {late} times in 20 s, long after the flood");
        assert_eq!(node.tip().await, honest.tip().await);
    });
}
