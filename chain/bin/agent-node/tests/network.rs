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
    let body = PostJob {
        m: 64,
        n: 64,
        k: 1024,
        config: pearl_job::MiningConfig::localnet_default().to_bytes(),
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
