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

    async fn balance(&self, who: &Pubkey) -> u64 {
        let v = self.call("getAccount", json!({"account": bs58::encode(who).into_string()})).await;
        v["balance"].as_str().unwrap().parse().unwrap()
    }
}

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
        };
        let prefix = format!("node{i}");
        let chain = Chain::open(ctx.child("chain").with_attribute("node", i), &prefix, genesis(k), MockVerifier).await.unwrap();
        let core = Core::new(chain);
        let (tx, rx) = mpsc::channel(256);
        let me = key.clone();
        let provider = oracle.manager();
        let blocker = ctl.clone();
        ctx.child("node").with_attribute("node", i).spawn(move |c| async move {
            run(c, core, me, provider, blocker, ch, rx, ActorConfig { tip_interval: Duration::from_millis(500) }).await;
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
