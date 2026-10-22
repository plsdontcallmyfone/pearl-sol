//! Multi-node tests on the OPEN network (net::open over commonware-stream), on the
//! deterministic runtime's in-memory sockets: nodes know only a seed address, find
//! each other by address gossip, and sync through the unchanged resolver / gossip
//! code. Blocks are mined through the JSON-RPC dispatcher with MockVerifier proofs.

use std::net::SocketAddr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, UNIX_EPOCH};

use agent_node::node::{run, ActorConfig, PeerCount};
use agent_types::tx::{deposit_message, pubkey_from_seed, sign_plain};
use agent_types::*;
use base64::Engine as _;
use commonware_cryptography::{ed25519, Signer as _};
use commonware_p2p::{Recipients, Sender as _};
use commonware_runtime::{deterministic, Clock, IoBuf, Runner as _, Spawner as _, Supervisor as _};
use consensus_pow::{Chain, Genesis, MockVerifier};
use futures::channel::{mpsc, oneshot};
use futures::SinkExt;
use net::open::testing::GatedNetwork;
use net::open::{Config as OpenConfig, Oracle, Seed};
use rpc::{ApiRequest, Core};
use serde_json::{json, Value};
use state::config::Config;

const CHAIN: u32 = 12;
const T0: u64 = 1_760_000_000_000;

fn seed(i: u8) -> [u8; 32] {
    [i; 32]
}
fn pk(i: u8) -> Pubkey {
    pubkey_from_seed(&seed(i))
}
fn addr(port: u16) -> SocketAddr {
    SocketAddr::from(([127, 0, 0, 1], port))
}
fn node_key(i: usize) -> ed25519::PublicKey {
    ed25519::PrivateKey::from_seed(300 + i as u64).public_key()
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

    async fn template(&self, miner: u8) -> (Header, Value) {
        let t = self.call("getBlockTemplate", json!({"miner": bs58::encode(pk(miner)).into_string(), "job_id": "fallback"})).await;
        (Header::decode(&hex::decode(t["header"].as_str().unwrap()).unwrap()).unwrap(), t)
    }

    async fn mine(&self, miner: u8) {
        let (mut h, t) = self.template(miner).await;
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
    }
}

struct Node {
    api: Api,
    oracle: Oracle,
    down: Arc<AtomicBool>,
}

fn open_cfg(i: usize, seeds: &[usize]) -> OpenConfig {
    let mut c = OpenConfig::local(ed25519::PrivateKey::from_seed(300 + i as u64), b"unused", addr(4000 + i as u16));
    c.seeds = seeds.iter().map(|s| Seed { addr: addr(4000 + *s as u16), key: None }).collect();
    c.dial_interval = Duration::from_millis(200);
    c.gossip_interval = Duration::from_secs(1);
    c.ping_interval = Duration::from_secs(1);
    c.idle_timeout = Duration::from_secs(5);
    c.backoff_base = Duration::from_millis(200);
    c.backoff_max = Duration::from_secs(2);
    c.full_backoff = Duration::from_secs(2);
    c.max_outbound = 4;
    c
}

async fn spawn_node(ctx: &deterministic::Context, i: usize, k: u32, mut cfg: OpenConfig) -> Node {
    let chain = Chain::open(ctx.child("chain").with_attribute("node", i), &format!("open{i}"), genesis(k), MockVerifier).await.unwrap();
    cfg.namespace = net::open_namespace(&chain.genesis_hash());
    let me = cfg.public_key();
    let (gated, down) = GatedNetwork::new(ctx.child("transport").with_attribute("node", i));
    let started = net::start_open(ctx.child("p2p").with_attribute("node", i), gated, cfg);
    let oracle = started.oracle.clone();
    let core = Core::new(chain);
    let (tx, rx) = mpsc::channel(256);
    let (o1, o2) = (oracle.clone(), oracle.clone());
    let count = oracle.clone();
    let info = oracle.clone();
    let acfg = ActorConfig {
        tip_interval: Duration::from_millis(500),
        peer_count: Some(PeerCount(Arc::new(move || count.connected_count()))),
        peer_info: Some(Arc::new(move || agent_node::node::open_peers_json(&info))),
    };
    ctx.child("node").with_attribute("node", i).spawn(move |c| async move {
        run(c, core, me, o1, o2, started.channels, rx, acfg).await;
    });
    Node { api: Api(tx), oracle, down }
}

fn runner(s: u64) -> deterministic::Runner {
    let s = s + std::env::var("NET_TEST_SEED").ok().and_then(|v| v.parse::<u64>().ok()).unwrap_or(0) * 1000;
    deterministic::Runner::new(
        deterministic::Config::new()
            .with_seed(s)
            .with_start_time(UNIX_EPOCH + Duration::from_millis(T0 + 1000))
            .with_timeout(Some(Duration::from_secs(3600))),
    )
}

async fn wait_tips_equal(ctx: &deterministic::Context, nodes: &[&Node], secs: u64) -> (String, u64) {
    for _ in 0..secs * 10 {
        let mut tips = Vec::new();
        for n in nodes {
            tips.push(n.api.tip().await);
        }
        if tips.iter().all(|t| *t == tips[0]) {
            return tips[0].clone();
        }
        ctx.sleep(Duration::from_millis(100)).await;
    }
    let mut tips = Vec::new();
    for n in nodes {
        tips.push(n.api.tip().await);
    }
    panic!("tips did not converge: {tips:?}");
}

#[test]
fn nodes_join_knowing_only_a_seed_and_converge() {
    runner(1).start(|ctx| async move {
        let n0 = spawn_node(&ctx, 0, 10, open_cfg(0, &[])).await;
        for _ in 0..3 {
            n0.api.mine(9).await;
        }
        let n1 = spawn_node(&ctx, 1, 10, open_cfg(1, &[0])).await;
        let n2 = spawn_node(&ctx, 2, 10, open_cfg(2, &[0])).await;
        wait_tips_equal(&ctx, &[&n0, &n1, &n2], 30).await;
        n2.api.mine(8).await;
        wait_tips_equal(&ctx, &[&n0, &n1, &n2], 30).await;
        n1.api.mine(7).await;
        // late joiners sync the whole chain (headers first) from whoever they meet
        let n3 = spawn_node(&ctx, 3, 10, open_cfg(3, &[0])).await;
        let n4 = spawn_node(&ctx, 4, 10, open_cfg(4, &[0])).await;
        let all = [&n0, &n1, &n2, &n3, &n4];
        let (_, h) = wait_tips_equal(&ctx, &all, 60).await;
        assert_eq!(h, 5);
        n4.api.mine(6).await;
        let (_, h) = wait_tips_equal(&ctx, &all, 30).await;
        assert_eq!(h, 6);
        ctx.sleep(Duration::from_secs(5)).await;
        for (i, n) in all.iter().enumerate().skip(1) {
            let peers = n.oracle.peers();
            assert!(peers.iter().any(|p| p.key != node_key(0)), "node {i} found no peer besides the seed");
        }
        // getChainInfo reports live connections
        let info = n4.api.call("getChainInfo", json!({})).await;
        assert_eq!(info["peers"].as_u64().unwrap() as usize, n4.oracle.connected_count());
        // getPeers lists each live connection with its direction and addresses
        let gp = n4.api.call("getPeers", json!({})).await;
        assert_eq!(gp["mode"], json!("open"));
        assert_eq!(gp["count"].as_u64().unwrap() as usize, n4.oracle.connected_count());
        let peers = gp["peers"].as_array().unwrap();
        assert_eq!(peers.len(), n4.oracle.connected_count());
        assert!(peers.iter().all(|p| p["key"].is_string() && p["remote"].is_string()));
        assert!(peers.iter().any(|p| p["direction"] == json!("outbound")));
        // a tx sent to node 3 reaches node 1's mempool through gossip and is mined there
        let msg = deposit_message(1, &pk(2), 77);
        let sig = AttesterSig { pubkey: pk(50), sig: sign_plain(&seed(50), &msg) };
        let tx = Tx::signed(&seed(200), CHAIN, 0, 0, TxBody::BridgeUnlock { solana_nonce: 1, recipient: pk(2), amount_spl: 77, job_order: None, sigs: vec![sig] });
        n3.api.call("sendTx", json!({"tx": base64::engine::general_purpose::STANDARD.encode(tx.encode())})).await;
        let mut seen = false;
        for _ in 0..50 {
            if n1.api.call("getMempool", json!({})).await["size"] == json!(1) {
                seen = true;
                break;
            }
            ctx.sleep(Duration::from_millis(100)).await;
        }
        assert!(seen, "tx did not reach node 1");
        n1.api.mine(9).await;
        let (_, h) = wait_tips_equal(&ctx, &all, 30).await;
        assert_eq!(h, 7);
        let acc = n0.api.call("getAccount", json!({"account": bs58::encode(pk(2)).into_string()})).await;
        assert_eq!(acc["balance"], json!("77000"));
    });
}

#[test]
fn node_sending_invalid_blocks_gets_banned() {
    runner(2).start(|ctx| async move {
        let n0 = spawn_node(&ctx, 0, 10, open_cfg(0, &[])).await;
        let n1 = spawn_node(&ctx, 1, 10, open_cfg(1, &[0])).await;
        n0.api.mine(9).await;
        wait_tips_equal(&ctx, &[&n0, &n1], 30).await;
        // the attacker is a bare open-network endpoint with the node's channels
        let mut ecfg = open_cfg(9, &[0]);
        let g = Chain::open(ctx.child("chain").with_attribute("node", 99), "evil", genesis(10), MockVerifier).await.unwrap();
        ecfg.namespace = net::open_namespace(&g.genesis_hash());
        let (gated, _) = GatedNetwork::new(ctx.child("transport").with_attribute("node", 9));
        let evil = net::start_open(ctx.child("p2p").with_attribute("node", 9), gated, ecfg);
        ctx.sleep(Duration::from_secs(3)).await;
        let ek = node_key(9);
        assert!(n0.oracle.is_connected(&ek), "attacker connected");
        // a block on the real tip with a bogus proof of work
        let (mut h, tj) = n0.api.template(66).await;
        let proof = vec![0xAA; 48];
        h.proof_hash = block::proof_hash(&proof);
        let coinbase = Some(Coinbase::decode(&hex::decode(tj["coinbase"].as_str().unwrap()).unwrap()).unwrap());
        let bad = Block { header: h, proof, txs: vec![], coinbase, contracts: None, uncles: Vec::new() };
        let before = n0.api.tip().await;
        let mut s = evil.channels.blocks.0.clone();
        let sent = s.send(Recipients::One(node_key(0)), IoBuf::from(bad.encode()), false);
        assert_eq!(sent.len(), 1);
        ctx.sleep(Duration::from_secs(2)).await;
        assert!(n0.oracle.is_banned(&ek), "invalid PoW block sender is banned");
        assert!(!n0.oracle.is_connected(&ek));
        assert_eq!(n0.api.tip().await, before, "the bad block changed nothing");
        // it keeps redialing the seed and is refused; honest nodes keep working
        ctx.sleep(Duration::from_secs(10)).await;
        assert!(!n0.oracle.is_connected(&ek));
        assert!(evil.oracle.stats().dial_failures > 0 || !evil.oracle.is_connected(&node_key(0)));
        assert!(n1.oracle.is_connected(&node_key(0)));
        n1.api.mine(8).await;
        let (_, hgt) = wait_tips_equal(&ctx, &[&n0, &n1], 30).await;
        assert_eq!(hgt, 2);
        // undecodable bytes on the blocks channel are also a ban (node 1 bans the
        // attacker if it ever reaches it)
        if n1.oracle.is_connected(&ek) {
            let _ = s.send(Recipients::One(node_key(1)), IoBuf::from(vec![1u8, 2, 3]), false);
            ctx.sleep(Duration::from_secs(2)).await;
            assert!(n1.oracle.is_banned(&ek));
        }
    });
}

#[test]
fn seed_inbound_cap_holds_and_partitioned_node_rejoins_via_gossip() {
    runner(3).start(|ctx| async move {
        let mut c0 = open_cfg(0, &[]);
        c0.max_inbound = 2;
        let n0 = spawn_node(&ctx, 0, 10, c0).await;
        let mut nodes = vec![n0];
        for i in 1..5 {
            let mut c = open_cfg(i, &[0]);
            c.max_outbound = 3;
            nodes.push(spawn_node(&ctx, i, 10, c).await);
            ctx.sleep(Duration::from_millis(500)).await;
        }
        nodes[1].api.mine(9).await;
        for _ in 0..100 {
            assert!(nodes[0].oracle.stats().inbound <= 2);
            ctx.sleep(Duration::from_millis(100)).await;
        }
        {
            let all: Vec<&Node> = nodes.iter().collect();
            wait_tips_equal(&ctx, &all, 30).await;
        }
        assert!(nodes[0].oracle.stats().inbound_full > 0, "the cap turned someone away");
        // node 4 drops off the network; the seed goes down for good
        nodes[4].down.store(true, Ordering::SeqCst);
        nodes[0].down.store(true, Ordering::SeqCst);
        ctx.sleep(Duration::from_secs(8)).await;
        assert_eq!(nodes[4].oracle.connected_count(), 0);
        // both sides mine while split (node 4 a short private branch)
        nodes[4].api.mine(4).await;
        for _ in 0..3 {
            nodes[2].api.mine(2).await;
            ctx.sleep(Duration::from_millis(300)).await;
        }
        let rest: Vec<&Node> = nodes[1..4].iter().collect();
        let (tip, h) = wait_tips_equal(&ctx, &rest, 30).await;
        assert_eq!(h, 4);
        assert_eq!(nodes[4].api.tip().await.1, 2);
        // heal node 4: the only seed is gone, so it must reconnect through addresses it
        // learned by gossip, then reorg onto the heavier branch
        nodes[4].down.store(false, Ordering::SeqCst);
        let mut synced = false;
        for _ in 0..300 {
            if nodes[4].api.tip().await == (tip.clone(), h) {
                synced = true;
                break;
            }
            ctx.sleep(Duration::from_millis(100)).await;
        }
        assert!(synced, "node 4 did not rejoin: tip {:?}, peers {:?}", nodes[4].api.tip().await, nodes[4].oracle.peers());
        let peers: Vec<_> = nodes[4].oracle.peers().into_iter().map(|p| p.key).collect();
        assert!(!peers.is_empty() && !peers.contains(&node_key(0)));
    });
}
