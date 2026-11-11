//! Header walks in batches (2026-11-11), on the simulated network.
//!
//! - A fresh node reaches a peer's tip with a handful of header requests instead of one per
//!   block, and says in getChainInfo that it is syncing and how far it is.
//! - A node syncs from a peer that only serves single headers (a node from before batches),
//!   and a node from before batches syncs from a new one: the three devnet boxes are upgraded
//!   one at a time.
//! - A peer that answers with a batch nobody asked for, an oversized one, one with a broken
//!   link, or one that is not the chain asked for is refused and blocked, nothing of the
//!   answer is held, and the node still syncs from the honest peer.
//! Harness copied from sync_flood.rs.

#![allow(dead_code)]

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
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
    /// Control of the last key: a bare p2p participant the test drives.
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

/// Two real nodes and one more key on the network with no node behind it. `batches[i]` is
/// node i's `header_batches` (false: a node from before batches).
async fn setup(ctx: &deterministic::Context, k: u32, batches: [bool; 2]) -> Net {
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
            run(c, core, me, provider, blocker, ch, rx, ActorConfig { tip_interval: Duration::from_millis(500), header_batches: batches[i], ..Default::default() }).await;
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


async fn sync_stats(a: &Api) -> Value {
    a.call("getNodeStats", json!({})).await["sync"].clone()
}

fn n(v: &Value, k: &str) -> u64 {
    v[k].as_u64().unwrap_or_else(|| panic!("no {k} in {v}"))
}

/// Mine `count` blocks on `a`, one a second.
async fn mine_chain(ctx: &deterministic::Context, a: &Api, count: u64) {
    for _ in 0..count {
        a.mine(9).await;
        ctx.sleep(Duration::from_secs(1)).await;
    }
}

/// The speed-up, counted: a fresh node walks the headers of a 1,200 block chain with 3 batch
/// requests and at most 5 single ones, where the walk of single headers needs one round trip
/// per header (the next test counts those on the same harness: 300 for 300). Its getChainInfo says it is syncing, with
/// the best header height, until it is at the tip.
#[test]
fn a_fresh_node_walks_headers_in_batches_and_reports_progress() {
    runner(41).start(|ctx| async move {
        const BLOCKS: u64 = 1_200;
        let net = setup(&ctx, 30, [true, true]).await;
        let (peer, node) = (&net.apis[0], &net.apis[1]);
        mine_chain(&ctx, peer, BLOCKS).await;
        let info = node.call("getChainInfo", json!({})).await;
        assert_eq!((info["height"].as_u64(), info["syncing"].as_bool(), info["best_header_height"].as_u64()), (Some(0), Some(false), Some(0)), "{info}");
        assert_eq!(info["sync"]["phase"], json!("synced"));

        net.link(0, 1).await;
        // watch getChainInfo while it syncs
        let mut phases: Vec<String> = Vec::new();
        let mut reports = 0;
        let mut last_height = 0;
        for _ in 0..3_000 {
            let i = node.call("getChainInfo", json!({})).await;
            let (height, best) = (i["height"].as_u64().unwrap(), i["best_header_height"].as_u64().unwrap());
            let phase = i["sync"]["phase"].as_str().unwrap().to_string();
            assert_eq!(i["syncing"].as_bool().unwrap(), phase != "synced", "{i}");
            assert!(best >= height && best <= BLOCKS, "{i}");
            assert!(height >= last_height);
            last_height = height;
            assert_eq!(i["sync"]["block_height"].as_u64(), Some(height));
            assert_eq!(i["sync"]["blocks_behind"].as_u64(), Some(best - height));
            if phase != "synced" {
                // while it syncs, the height it is heading for is on show
                assert!(best > height + 1, "{i}");
                reports += 1;
                if phase == "headers" {
                    let low = i["sync"]["header_walk_low"].as_u64().expect("the walk says how far down it is");
                    assert!(low > height && low <= best, "{i}");
                }
                if phase == "bodies" {
                    assert_eq!(best, BLOCKS, "the header chain is complete before bodies: {i}");
                    assert!(i["sync"]["headers_held"].as_u64().unwrap() > 0);
                }
            }
            if phases.last() != Some(&phase) {
                phases.push(phase);
            }
            if height == BLOCKS {
                break;
            }
            ctx.sleep(Duration::from_millis(20)).await;
        }
        assert_eq!(node.tip().await, peer.tip().await, "the fresh node did not reach the tip");
        assert!(reports > 5, "getChainInfo reported syncing {reports} times");
        assert!(phases.contains(&"bodies".to_string()), "{phases:?}");
        assert_eq!(phases.last().map(String::as_str), Some("synced"), "{phases:?}");
        assert_eq!(phases.first().map(String::as_str), Some("synced"), "before the first header: {phases:?}");
        let order: Vec<&str> = phases.iter().map(String::as_str).filter(|p| *p != "synced").collect();
        assert!(order == ["headers", "bodies"] || order == ["bodies"], "headers, then bodies: {phases:?}");

        // the count: 1,199 headers below the tip in 3 batches (512 + 512 + what is left above
        // the single-header zone near the node's own chain)
        let s = sync_stats(node).await;
        let served = sync_stats(peer).await;
        assert_eq!(n(&s, "batches_delivered"), 3, "{s}");
        assert_eq!(n(&s, "batch_asks"), 3, "{s}");
        assert_eq!(n(&s, "batch_asks_expired") + n(&s, "batches_refused"), 0, "{s}");
        assert_eq!(n(&served, "batches_served"), 3, "{served}");
        assert!(n(&s, "batch_headers") >= BLOCKS - 40, "{s}");
        let round_trips = n(&s, "batch_asks") + n(&s, "header_asks");
        assert!(round_trips <= 3 + 5, "{round_trips} header requests for {BLOCKS} headers: {s}");
        println!("header sync of {BLOCKS} blocks: {} batch asks + {} single asks = {round_trips} requests ({s})", n(&s, "batch_asks"), n(&s, "header_asks"));

        // at the tip it follows new blocks with single headers and is not syncing
        peer.mine(9).await;
        let after = wait_synced(&ctx, &net.apis, 20, "the node stopped following the tip").await;
        assert_eq!(after.1, BLOCKS + 1);
        ctx.sleep(Duration::from_secs(2)).await;
        let i = node.call("getChainInfo", json!({})).await;
        assert_eq!((i["syncing"].as_bool(), i["best_header_height"].as_u64()), (Some(false), Some(BLOCKS + 1)), "{i}");
        assert_eq!(n(&sync_stats(node).await, "batch_asks"), 3);
    });
}

/// Upgrade order, first box: a new node among peers from before batches. Its one batch ask
/// goes unanswered (the old peer says "do not have it", as for any unknown hash) and it syncs
/// with single headers, one round trip per header, as before. The old peer is not upset: both
/// keep following each other afterwards. And the other way round: a node from before batches
/// syncs from a new one with single headers.
#[test]
fn a_new_node_and_a_node_without_batches_sync_from_each_other() {
    runner(42).start(|ctx| async move {
        const BLOCKS: u64 = 300;
        // node 0: before batches, holds the chain. node 1: new, fresh.
        let net = setup(&ctx, 30, [false, true]).await;
        let (old, new) = (&net.apis[0], &net.apis[1]);
        mine_chain(&ctx, old, BLOCKS).await;
        net.link(0, 1).await;
        let synced = wait_synced(&ctx, &net.apis, 600, "the new node did not sync from the peer without batches").await;
        assert_eq!(synced.1, BLOCKS);
        ctx.sleep(Duration::from_secs(10)).await; // past the batch ask timeout
        let s = sync_stats(new).await;
        assert_eq!(n(&s, "batches_delivered"), 0, "{s}");
        assert!(n(&s, "batch_asks") >= 1, "it tried a batch: {s}");
        assert!(n(&s, "batch_asks") <= 3, "one probe at a time, one per interval: {s}");
        assert_eq!(n(&s, "batch_asks"), n(&s, "batch_asks_expired"), "{s}");
        assert!(n(&s, "headers_delivered") >= BLOCKS, "one single header per block: {s}");
        assert!(n(&s, "header_asks") >= BLOCKS && n(&s, "header_asks") <= BLOCKS + 8, "{s}");
        let o = sync_stats(old).await;
        assert_eq!((n(&o, "batches_served"), n(&o, "batch_asks")), (0, 0), "{o}");
        println!("header sync of {BLOCKS} blocks from a peer without batches: {} single asks ({s})", n(&s, "header_asks"));
        // neither side blocked the other: blocks mined on either are followed by both
        old.mine(9).await;
        assert_eq!(wait_synced(&ctx, &net.apis, 20, "the new node stopped following the old one").await.1, BLOCKS + 1);
        ctx.sleep(Duration::from_secs(2)).await;
        new.mine(8).await;
        assert_eq!(wait_synced(&ctx, &net.apis, 20, "the old node stopped following the new one").await.1, BLOCKS + 2);
    });
    runner(43).start(|ctx| async move {
        const BLOCKS: u64 = 300;
        // node 0: new, holds the chain. node 1: before batches, fresh.
        let net = setup(&ctx, 30, [true, false]).await;
        let (new, old) = (&net.apis[0], &net.apis[1]);
        mine_chain(&ctx, new, BLOCKS).await;
        net.link(0, 1).await;
        let synced = wait_synced(&ctx, &net.apis, 600, "the node without batches did not sync from the new one").await;
        assert_eq!(synced.1, BLOCKS);
        let o = sync_stats(old).await;
        assert_eq!((n(&o, "batch_asks"), n(&o, "batches_delivered")), (0, 0), "{o}");
        assert!(n(&o, "header_asks") >= BLOCKS, "{o}");
        assert_eq!(n(&sync_stats(new).await, "batches_served"), 0);
    });
}

/// A linked chain of `count` junk job headers on an unknown parent, lowest first.
fn junk_chain(seed: u64, count: usize, top_height: u64) -> Vec<(Header, Vec<u8>)> {
    let mut prev = [0xee; 32];
    prev[..8].copy_from_slice(&seed.to_le_bytes());
    let mut out = Vec::new();
    for i in 0..count as u64 {
        let mut proof = vec![0u8; 16];
        proof[..8].copy_from_slice(&(seed ^ i).to_le_bytes());
        let height = top_height + 1 - count as u64 + i;
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
        prev = header.hash();
        out.push((header, proof));
    }
    out
}

fn batch_of(headers: &[(Header, Vec<u8>)]) -> Vec<u8> {
    consensus_pow::encode_header_batch(&headers.iter().map(|(h, p)| encode_header_with_proof(h, p)).collect::<Vec<_>>())
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Bad {
    /// A batch as the answer to a request for one header.
    Unasked,
    /// 513 linked headers.
    TooMany,
    /// Linked headers with 16 KB proofs, over the byte bound.
    TooLarge,
    /// The chain asked for with one entry missing in the middle.
    BrokenLink,
    /// Real, valid, linked headers of the honest chain, which is not the chain asked for.
    OtherChain,
}

/// What the misbehaving peer saw.
#[derive(Default)]
struct Seen {
    /// Requests it answered with its bad batch.
    bad_answers: AtomicU64,
    /// Every request that reached it.
    requests: AtomicU64,
}

/// One misbehaving peer, one kind of bad answer. The peer announces a made-up tip 600 blocks
/// above the node and serves its header; the node walks down from it and asks for a batch (or,
/// for `Unasked`, the peer answers the single request for the tip with a batch). Whatever the
/// kind: the answer is refused, nothing of it is held, the peer is blocked (no request reaches
/// it any more), and the node syncs from the honest peer.
fn bad_batch_case(bad: Bad, seed: u64) {
    runner(seed).start(move |ctx| async move {
        let net = setup(&ctx, 30, [true, true]).await;
        let (honest, node) = (&net.apis[0], &net.apis[1]);
        net.link(0, 1).await;
        ctx.sleep(Duration::from_secs(1)).await;
        for _ in 0..5 {
            honest.mine(9).await;
        }
        assert_eq!(wait_synced(&ctx, &net.apis, 30, "the shared start did not sync").await.1, 5);
        net.unlink(0, 1).await;
        mine_chain(&ctx, honest, 60).await;
        let honest_tip = honest.tip().await;

        // the made-up chain: 600 linked junk headers, tip at height 605
        let junk = junk_chain(bad as u64 + 1, 600, 605);
        let (tip, tip_proof) = junk[599].clone();
        let parent = junk[598].0.hash();
        // newest first from the tip's parent, as a batch for it carries them
        let below: Vec<(Header, Vec<u8>)> = junk[..599].iter().rev().cloned().collect();
        let answer: Vec<u8> = match bad {
            Bad::Unasked => batch_of(&junk.iter().rev().take(100).cloned().collect::<Vec<_>>()),
            Bad::TooMany => batch_of(&below[..513]),
            Bad::TooLarge => {
                // 199 linked headers with 16 KB proofs: 3 MiB
                let mut fat: Vec<(Header, Vec<u8>)> = junk[400..599].to_vec();
                let mut prev = fat[0].0.prev_hash;
                for (h, p) in fat.iter_mut() {
                    p.resize(16 << 10, 0xab);
                    h.prev_hash = prev;
                    h.proof_hash = block::proof_hash(p);
                    prev = h.hash();
                }
                fat.reverse();
                batch_of(&fat)
            }
            Bad::BrokenLink => {
                let mut b = below[..200].to_vec();
                b.remove(100);
                batch_of(&b)
            }
            Bad::OtherChain => {
                let h = hex::decode(&honest_tip.0).unwrap();
                let mut real = Vec::new();
                let mut cur: Hash = h.try_into().unwrap();
                for _ in 0..40 {
                    let v = honest.call("getHeader", json!({"hash": hex::encode(cur)})).await;
                    let header = Header::decode(&hex::decode(v["header"].as_str().unwrap()).unwrap()).unwrap();
                    let proof = base64::engine::general_purpose::STANDARD.decode(v["proof"].as_str().unwrap()).unwrap();
                    cur = header.prev_hash;
                    real.push((header, proof));
                }
                batch_of(&real)
            }
        };
        if bad == Bad::TooLarge {
            assert!(answer.len() > consensus_pow::MAX_HEADER_BATCH_BYTES);
        }
        let tip_hash = tip.hash();
        let tip_payload = encode_header_with_proof(&tip, &tip_proof);
        let batch_key = sync::batch_key(&parent);

        let seen = Arc::new(Seen::default());
        let q = net::quota();
        let ctl = net.flooder.clone();
        let (mut tips_tx, _tips_rx) = ctl.clone().register(net::CH_TIPS, q).await.unwrap();
        let headers_ch = ctl.clone().register(net::CH_HEADERS, q).await.unwrap();
        let bodies_ch = ctl.clone().register(net::CH_BODIES, q).await.unwrap();
        let (ev_tx, mut ev_rx) = commonware_utils::channel::mpsc::unbounded_channel::<SyncEvent>();
        let _headers = sync::start(ctx.child("bad_headers"), Kind::Header, net.keys[2].clone(), net.oracle.manager(), ctl.clone(), headers_ch, ev_tx.clone());
        let _bodies = sync::start(ctx.child("bad_bodies"), Kind::Body, net.keys[2].clone(), net.oracle.manager(), ctl.clone(), bodies_ch, ev_tx);
        {
            let seen = seen.clone();
            ctx.child("bad").spawn(move |_| async move {
                while let Some(ev) = ev_rx.recv().await {
                    if let SyncEvent::Serve { kind: Kind::Header, key, reply } = ev {
                        seen.requests.fetch_add(1, Ordering::Relaxed);
                        if key == tip_hash && bad == Bad::Unasked {
                            seen.bad_answers.fetch_add(1, Ordering::Relaxed);
                            let _ = reply.send(Bytes::from(answer.clone()));
                        } else if key == tip_hash {
                            let _ = reply.send(Bytes::from(tip_payload.clone()));
                        } else if key == batch_key {
                            seen.bad_answers.fetch_add(1, Ordering::Relaxed);
                            let _ = reply.send(Bytes::from(answer.clone()));
                        }
                    }
                }
            });
        }
        net.link(1, 2).await;
        ctx.sleep(Duration::from_millis(500)).await;
        let mut m = tip_hash.to_vec();
        m.extend_from_slice(&605u64.to_le_bytes());
        let _ = tips_tx.send(Recipients::One(net.keys[1].clone()), IoBuf::from(m.clone()), false);
        ctx.sleep(Duration::from_secs(3)).await;

        assert_eq!(seen.bad_answers.load(Ordering::Relaxed), 1, "{bad:?}: the peer got to send its bad batch once");
        let s = sync_stats(node).await;
        let info = node.call("getChainInfo", json!({})).await;
        assert_eq!(n(&s, "batches_delivered") + n(&s, "batch_headers"), 0, "{bad:?}: {s}");
        if bad == Bad::Unasked {
            assert_eq!(info["sync"]["headers_held"].as_u64(), Some(0), "{bad:?}: {info}");
        } else {
            assert_eq!(n(&s, "batches_refused"), 1, "{bad:?}: {s}");
            assert_eq!(info["sync"]["headers_held"].as_u64(), Some(1), "{bad:?}: only the tip header it served singly: {info}");
        }
        assert_eq!(info["height"].as_u64(), Some(5));

        // blocked: it announces again and again, and no request reaches it any more
        let before = seen.requests.load(Ordering::Relaxed);
        for i in 0..20u64 {
            let mut other = [0x77u8; 32];
            other[..8].copy_from_slice(&i.to_le_bytes());
            let mut t = other.to_vec();
            t.extend_from_slice(&700u64.to_le_bytes());
            let _ = tips_tx.send(Recipients::One(net.keys[1].clone()), IoBuf::from(t), false);
            let _ = tips_tx.send(Recipients::One(net.keys[1].clone()), IoBuf::from(m.clone()), false);
            ctx.sleep(Duration::from_millis(500)).await;
        }
        assert_eq!(seen.requests.load(Ordering::Relaxed), before, "{bad:?}: the node still asks the peer it refused");

        // the honest peer comes back: the node syncs, with the refused peer still connected
        net.link(0, 1).await;
        let synced = wait_synced(&ctx, &net.apis, 60, "the node did not sync from the honest peer").await;
        assert_eq!(synced, honest_tip, "{bad:?}");
        honest.mine(9).await;
        assert_eq!(wait_synced(&ctx, &net.apis, 20, "the node stopped following the tip").await.1, 66);
        let s = sync_stats(node).await;
        // the refused peer's pause is its own: the honest peer's walk got its batch
        assert_eq!(n(&s, "batches_delivered"), 1, "{bad:?}: {s}");
        assert!(n(&s, "batch_headers") >= 58, "{bad:?}: the 59 honest headers below the tip in one batch: {s}");
    });
}

#[test]
fn a_batch_nobody_asked_for_is_refused() {
    bad_batch_case(Bad::Unasked, 51);
}

#[test]
fn an_oversized_batch_is_refused() {
    bad_batch_case(Bad::TooMany, 52);
    bad_batch_case(Bad::TooLarge, 53);
}

#[test]
fn a_batch_with_a_broken_link_is_refused() {
    bad_batch_case(Bad::BrokenLink, 54);
}

#[test]
fn a_batch_that_is_not_the_chain_asked_for_is_refused() {
    bad_batch_case(Bad::OtherChain, 55);
}
