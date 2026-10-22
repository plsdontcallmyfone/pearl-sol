//! Blob data lifecycle (SPEC 4.5 "Segment data retention", 2026-10-22 devnet fix): which
//! segment A blobs a node keeps, by chain state (mined, final, ticket closed, replay window,
//! mining window), model blobs pinned for good, RPC put backpressure near the quota, a reorg
//! around a mined and around a pruned segment, and a fresh node syncing after pruning. Real
//! node actors on the deterministic runtime and simulated p2p, blocks through the JSON-RPC
//! dispatcher with MockVerifier proofs (the same harness as tests/network.rs).

use std::time::{Duration, UNIX_EPOCH};

use agent_node::node::{run, ActorConfig};
use agent_types::tx::{deposit_message, pubkey_from_seed, result_message, sign_plain};
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
use pearl_job::standing;
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
fn b64e(b: &[u8]) -> String {
    base64::engine::general_purpose::STANDARD.encode(b)
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
    async fn try_call(&self, method: &str, params: Value) -> Result<Value, String> {
        let (tx, rx) = oneshot::channel();
        self.0.clone().send(ApiRequest::Call { method: method.into(), params, reply: tx }).await.unwrap();
        rx.await.unwrap().map_err(|e| e.message)
    }
    async fn call(&self, method: &str, params: Value) -> Value {
        self.try_call(method, params).await.unwrap_or_else(|e| panic!("{method}: {e}"))
    }
    async fn height(&self) -> u64 {
        self.call("getChainInfo", json!({})).await["height"].as_u64().unwrap()
    }
    async fn info(&self) -> Value {
        self.call("getChainInfo", json!({})).await
    }
    /// Mine one block: `job` "fallback", None (auto) or a job id.
    async fn mine_with(&self, job: Option<&str>) -> Value {
        let mut q = json!({"miner": bs58::encode(pk(9)).into_string()});
        if let Some(j) = job {
            q["job_id"] = json!(j);
        }
        let t = self.call("getBlockTemplate", q).await;
        let mut h = Header::decode(&hex::decode(t["header"].as_str().unwrap()).unwrap()).unwrap();
        let proof = MockVerifier::proof_for(&h);
        h.proof_hash = block::proof_hash(&proof);
        let r = self
            .call("submitBlock", json!({"header": hex::encode(h.encode()), "proof": b64e(&proof), "template_id": t["template_id"]}))
            .await;
        assert_eq!(r["accepted"], json!(true), "{r}");
        t
    }
    async fn mine(&self) {
        self.mine_with(Some("fallback")).await;
    }
    async fn mine_n(&self, n: u64) {
        for _ in 0..n {
            self.mine().await;
        }
    }
    async fn send(&self, tx: &Tx) {
        self.try_call("sendTx", json!({"tx": b64e(&tx.encode())})).await.unwrap_or_else(|e| panic!("sendTx: {e}"));
    }
    async fn has(&self, id: &str) -> bool {
        self.call("hasBlob", json!({"blob_id": id})).await["present"] == json!(true)
    }
    async fn fetching(&self, id: &str) -> bool {
        self.call("hasBlob", json!({"blob_id": id})).await["fetching"] == json!(true)
    }
    async fn put(&self, data: &[u8]) -> String {
        self.call("putBlob", json!({"data": b64e(data)})).await["blob_id"].as_str().unwrap().to_string()
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

/// `n` nodes with finality `k`; node `i` gets the blob config `cfg(i)`; none linked.
async fn setup(ctx: &deterministic::Context, n: usize, k: u32, cfg: impl Fn(usize) -> blobs::BlobConfig) -> Net {
    let (network, oracle) = Network::new(
        ctx.child("network"),
        NetConfig { max_size: 8 << 20, max_peers_per_set: std::num::NonZeroUsize::new(n).unwrap(), disconnect_on_block: false, tracked_peer_sets: std::num::NonZeroUsize::new(3).unwrap() },
    );
    network.start();
    let signers: Vec<ed25519::PrivateKey> = (0..n).map(|i| ed25519::PrivateKey::from_seed(300 + i as u64)).collect();
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
        let store = blobs::BlobStore::open(ctx.child("blobs").with_attribute("node", i), &format!("{prefix}-blobs"), cfg(i), 0).await.unwrap();
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
    Net { oracle, keys, apis }
}

fn runner(seed: u64) -> deterministic::Runner {
    deterministic::Runner::new(
        deterministic::Config::new()
            .with_seed(seed)
            .with_start_time(UNIX_EPOCH + Duration::from_millis(T0 + 1000))
            .with_timeout(Some(Duration::from_secs(7200))),
    )
}

async fn wait_tips_equal(ctx: &deterministic::Context, apis: &[Api], secs: u64) -> (String, u64) {
    for _ in 0..secs * 10 {
        let mut tips = Vec::new();
        for a in apis {
            let i = a.info().await;
            tips.push((i["tip"].as_str().unwrap().to_string(), i["height"].as_u64().unwrap()));
        }
        if tips.iter().all(|t| *t == tips[0]) {
            return tips[0].clone();
        }
        ctx.sleep(Duration::from_millis(100)).await;
    }
    panic!("tips did not converge");
}

async fn wait_blob(ctx: &deterministic::Context, api: &Api, id: &str, secs: u64) -> bool {
    for _ in 0..secs * 10 {
        if api.has(id).await {
            return true;
        }
        ctx.sleep(Duration::from_millis(100)).await;
    }
    false
}

fn unlock_tx(rnonce: u64, solana_nonce: u64, to: Pubkey, amount_spl: u64) -> Tx {
    let msg = deposit_message(solana_nonce, &to, amount_spl);
    let sig = AttesterSig { pubkey: pk(50), sig: sign_plain(&seed(50), &msg) };
    Tx::signed(&seed(200), CHAIN, rnonce, 0, TxBody::BridgeUnlock { solana_nonce, recipient: to, amount_spl, job_order: None, sigs: vec![sig] })
}

const SERVER: u8 = 4;
const POSTER: u8 = 3;
const EVAL: u8 = 60;

/// A registered two-matrix model with its manifest and chunk uploaded (push false).
struct Model {
    id: Hash,
    manifest: String,
    chunk: String,
}

/// Fund the accounts, register a two-matrix model (manifest + one chunk), mined in.
async fn fund_and_model(api: &Api) -> Model {
    for (i, who) in [pk(1), pk(2), pk(POSTER), pk(SERVER), pk(EVAL), agent_types::pool_address()].into_iter().enumerate() {
        api.send(&unlock_tx(i as u64, i as u64 + 1, who, 1_000_000)).await;
    }
    api.mine().await;
    let chunk = vec![7u8; 1000];
    let chunk_id = hex::encode(blake3::hash(&chunk).as_bytes());
    let manifest = serde_json::to_vec(&json!({"format": "cadre-model-v1", "files": {"w/0.i8": {"size": 1000, "blake3": chunk_id, "blobs": [format!("blob:{chunk_id}")]}}})).unwrap();
    let manifest_id = hex::encode(blake3::hash(&manifest).as_bytes());
    let c = pearl_job::MiningConfig::localnet_default();
    let reg = RegisterModel {
        name: b"tiny".to_vec(),
        manifest_uri: format!("blob:{manifest_id}").into_bytes(),
        recipe: 1,
        rank: c.rank,
        rows_pattern: c.rows_pattern.to_bytes(),
        cols_pattern: c.cols_pattern.to_bytes(),
        matrices: vec![MatrixSpec { k: 2048, n: 8, hash_b: [0x61; 32] }, MatrixSpec { k: 2048, n: 8, hash_b: [0x62; 32] }],
    };
    api.send(&Tx::signed(&seed(2), CHAIN, 0, 10, TxBody::RegisterModel(reg))).await;
    api.mine().await;
    api.call("putBlob", json!({"data": b64e(&manifest), "push": false})).await;
    api.call("putBlob", json!({"data": b64e(&chunk), "push": false})).await;
    Model { id: standing::model_id(&pk(2), b"tiny"), manifest: manifest_id, chunk: chunk_id }
}

/// A ticket (poster nonce `nonce`, deadline `deadline`, `blocks` block budget) mined in.
async fn post_ticket(api: &Api, model: &Model, nonce: u64, deadline: u64, blocks: u32) -> Hash {
    let req = api.put(format!("{{\"format\":\"cadre-request-v1\",\"n\":{nonce}}}").as_bytes()).await;
    let t = PostJob {
        m: 15, n: 1, k: 1, config: standing::ticket_config(&model.id), hash_a: [5; 32], hash_b: pk(SERVER),
        data_uri: format!("blob:{req}").into_bytes(), worker_pay: 100_000, protocol_fee: 200_000, deadline_height: deadline, max_blocks: blocks,
        evaluators: vec![pk(EVAL)], threshold: 1,
    };
    api.send(&Tx::signed(&seed(POSTER), CHAIN, nonce, 10, TxBody::PostJob(t))).await;
    api.mine().await;
    state::schema::job_id_for_post(&pk(POSTER), nonce)
}

/// Round `round` of `tid` with one segment per matrix (`max_blocks` 1 each); A blobs put on
/// `api` (pushed to its peers). Returns `(segment id hex, A blob id hex)` per matrix.
async fn post_round(api: &Api, model: &Model, tid: Hash, round: u16, server_nonce: u64, salt: i8) -> Vec<(String, String)> {
    let c = pearl_job::MiningConfig::localnet_default();
    let mut segs = Vec::new();
    let mut out = Vec::new();
    for matrix in 0..2u16 {
        let a = vec![salt.wrapping_add(matrix as i8 * 3 + round as i8); 4 * 2048];
        let blob = standing::encode_activation_blob(4, 2048, &a).unwrap();
        let a_id = api.put(&blob).await;
        segs.push(Segment { matrix, m: 4, k: 2048, n: 8, max_blocks: 1, hash_a: [6; 32], hash_b: [0x61 + matrix as u8; 32], c_hash: [7; 32], a_uri: format!("blob:{a_id}").into_bytes() });
        out.push((hex::encode(standing::segment_id(&tid, matrix, round)), a_id));
    }
    let body = TxBody::PostSegments { ticket_id: tid, round, model_id: model.id, rank: c.rank, rows_pattern: c.rows_pattern.to_bytes(), cols_pattern: c.cols_pattern.to_bytes(), segments: segs };
    api.send(&Tx::signed(&seed(SERVER), CHAIN, server_nonce, 10, body)).await;
    api.mine().await;
    out
}

/// The server delivers and the evaluator completes the ticket (two blocks). Returns the close height.
async fn complete(api: &Api, tid: Hash, server_nonce: u64, eval_nonce: u64) -> u64 {
    let resp = format!("{{\"format\":\"cadre-response-v1\",\"t\":\"{}\"}}", hex::encode(tid));
    let rid = api.put(resp.as_bytes()).await;
    let rh: Hash = hex::decode(&rid).unwrap().try_into().unwrap();
    api.send(&Tx::signed(&seed(SERVER), CHAIN, server_nonce, 10, TxBody::SubmitResult { job_id: tid, result_hash: rh, uri: format!("blob:{rid}").into_bytes() })).await;
    api.mine().await;
    let msg = result_message(&tid, &pk(SERVER), &rh);
    api.send(&Tx::signed(&seed(EVAL), CHAIN, eval_nonce, 10, TxBody::CompleteJob { job_id: tid, sigs: vec![AttesterSig { pubkey: pk(EVAL), sig: sign_plain(&seed(EVAL), &msg) }] })).await;
    api.mine().await;
    let j = api.call("getJob", json!({"job_id": hex::encode(tid)})).await;
    assert_eq!(j["status"], json!("completed"), "{j}");
    j["closed_height"].as_u64().unwrap()
}

/// Mine until `id` is gone from `api` (at most `max` blocks); returns the height it went at.
async fn mine_until_gone(api: &Api, id: &str, max: u64) -> u64 {
    for _ in 0..max {
        if !api.has(id).await {
            return api.height().await;
        }
        api.mine().await;
    }
    panic!("blob {id} still held after {max} more blocks (tip {})", api.height().await);
}

const GRACE: u64 = 20;
const REPLAY: u32 = 30;
const MINE_WINDOW: u64 = 60;

/// The rule, state by state, on a replay-keeping node (0) and a mining-only node (1):
/// - a mined segment's A stays on the replay node while its ticket is open, and goes from the
///   mining-only node once the mined block is final (plus the grace period);
/// - after the ticket closes (final) the replay node drops it `segment_replay_blocks` later;
/// - an unmined segment's A stays for mining until `segment_mine_blocks` after posting;
/// - model blobs are never pruned; getBlobStats classes add up; getParams shows the window.
#[test]
fn segment_a_retention_by_state_and_model_blobs_pinned() {
    runner(41).start(|ctx| async move {
        let cfg = |i: usize| blobs::BlobConfig { grace_blocks: GRACE, segment_mine_blocks: MINE_WINDOW, keep_replay: i == 0, ..Default::default() };
        let net = setup(&ctx, 2, 3, cfg).await;
        net.link(0, 1).await;
        ctx.sleep(Duration::from_secs(1)).await;
        let api = &net.apis[0];
        let m = fund_and_model(api).await;
        // the replay window is a chain parameter (AdminConfig key 48, admin pk(1))
        api.send(&Tx::signed(&seed(1), CHAIN, 0, 10, TxBody::AdminConfig { key: 48, value: REPLAY.to_le_bytes().to_vec() })).await;
        api.mine().await;
        let p = api.call("getParams", json!({})).await;
        assert_eq!(p["segment_replay_blocks"], json!(REPLAY), "{p}");
        assert_eq!(p["keys"]["segment_replay_blocks"], json!(48));
        let tid = post_ticket(api, &m, 0, 400, 4).await;
        let segs = post_round(api, &m, tid, 0, 0, 1).await;
        let posted = api.height().await;
        let (s0, a0) = segs[0].clone();
        let (_s1, a1) = segs[1].clone();
        wait_tips_equal(&ctx, &net.apis, 20).await;
        assert!(wait_blob(&ctx, &net.apis[1], &a0, 20).await && wait_blob(&ctx, &net.apis[1], &a1, 20).await, "A pushed to the peer");
        // mine segment 0 only
        let t = api.mine_with(Some(&s0)).await;
        assert_eq!(t["job"]["kind"], json!("segment"));
        let mined_at = api.height().await;
        wait_tips_equal(&ctx, &net.apis, 20).await;
        assert_eq!(net.apis[1].call("getJob", json!({"job_id": s0})).await["blocks_mined"], json!(1));
        // segment 0 mined, ticket open: the replay node keeps A; the mining-only node drops it
        // once the block is final (K = 3) and the by-age grace (stored + 20) has passed
        let gone1 = mine_until_gone(&net.apis[1], &a0, 60).await;
        wait_tips_equal(&ctx, &net.apis, 20).await;
        assert!(gone1 > mined_at + 3, "kept while the mined block was not final ({gone1})");
        assert!(api.has(&a0).await, "replay node keeps a mined segment's A while the ticket is open");
        assert!(net.apis[1].has(&a1).await, "the unmined segment's A stays for mining");
        assert!(!net.apis[1].fetching(&a0).await, "the mining-only node does not fetch it back");
        // close the ticket: the replay node keeps A for REPLAY blocks after the close (final)
        let closed = complete(api, tid, 1, 0).await;
        let gone0 = mine_until_gone(api, &a0, 120).await;
        assert!(gone0 > closed + REPLAY as u64 + GRACE, "replay window: gone at {gone0}, closed at {closed}");
        assert!(gone0 <= closed + REPLAY as u64 + GRACE + 11, "pruned at the next pass after the window ({gone0}, closed {closed})");
        // the unmined segment 1: mineable until its mining window (posted + 60) ends; the replay
        // node also keeps it for the replay window, so it goes at the later of the two
        let gone_a1 = mine_until_gone(api, &a1, 120).await;
        let until = (posted + MINE_WINDOW).max(closed + REPLAY as u64);
        assert!(gone_a1 > until + GRACE && gone_a1 <= until + GRACE + 11, "unmined A gone at {gone_a1}, kept until {until} + grace");
        wait_tips_equal(&ctx, &net.apis, 20).await;
        assert!(!net.apis[1].has(&a1).await, "the mining-only node dropped it after its mining window too");
        // a segment past its mining window is no longer offered by this node's templates
        let t = api.call("getBlockTemplate", json!({"miner": bs58::encode(pk(9)).into_string()})).await;
        assert!(t["job"].is_null() || t["job"]["kind"] != json!("segment"), "{}", t["job"]);
        // model blobs: pinned through everything
        assert!(api.has(&m.manifest).await && api.has(&m.chunk).await, "model blobs are never pruned");
        let s = api.call("getBlobStats", json!({})).await;
        assert_eq!(s["classes"]["model"]["count"], json!(2), "{s}");
        assert_eq!(s["classes"]["model"]["prunable"], json!(0));
        assert_eq!(s["classes"]["segment_a"]["count"], json!(0), "{s}");
        let total: u64 = ["model", "segment_a", "social", "ticket", "job", "other"].iter().map(|k| s["classes"][*k]["bytes"].as_u64().unwrap()).sum();
        assert_eq!(total, s["total_bytes"].as_u64().unwrap());
        assert_eq!(s["retention"]["segment_replay_blocks"], json!(REPLAY));
        assert_eq!(s["retention"]["segment_mine_blocks"], json!(MINE_WINDOW));
        assert_eq!(s["retention"]["keep_replay"], json!(true));
        assert_eq!(net.apis[1].call("getBlobStats", json!({})).await["retention"]["keep_replay"], json!(false));
    });
}

/// getBlobStats while a segment is live: its A is referenced and kept, a loose upload is
/// "other" and prunable after the grace period; RPC puts stop at the reserve with a clear
/// error (backpressure) while the referenced share of the quota stays usable.
#[test]
fn blob_stats_breakdown_and_put_backpressure() {
    runner(42).start(|ctx| async move {
        // quota 200 KB, max blob 40 KB: the RPC reserve is 2 x 40 KB = 80 KB, puts stop at 120 KB
        let cfg = |_| blobs::BlobConfig { grace_blocks: GRACE, max_blob_bytes: 40_000, max_total_bytes: 200_000, ..Default::default() };
        let net = setup(&ctx, 1, 3, cfg).await;
        let api = &net.apis[0];
        let m = fund_and_model(api).await;
        let tid = post_ticket(api, &m, 0, 400, 4).await;
        // two 8 KB segment A blobs (4 rows x k 2048)
        let segs = post_round(api, &m, tid, 0, 0, 1).await;
        let loose = api.put(b"nobody references me").await;
        let s = api.call("getBlobStats", json!({})).await;
        assert_eq!(s["classes"]["segment_a"]["count"], json!(2), "{s}");
        assert_eq!(s["classes"]["segment_a"]["kept"], json!(2));
        assert_eq!(s["classes"]["model"]["count"], json!(2));
        assert_eq!(s["classes"]["ticket"]["count"], json!(1), "the request blob");
        assert_eq!(s["classes"]["other"]["count"], json!(1));
        assert_eq!(s["rpc_reserve_bytes"], json!(80_000));
        assert_eq!(s["rpc_put_limit_bytes"], json!(120_000));
        assert_eq!(s["puts_open"], json!(true));
        let used = s["total_bytes"].as_u64().unwrap();
        assert_eq!(s["rpc_headroom_bytes"].as_u64().unwrap(), 120_000 - used);
        // fill to the RPC limit: then a clear refusal
        let mut i = 0u8;
        let err = loop {
            let data = vec![100 + i; 39_000];
            match api.try_call("putBlob", json!({"data": b64e(&data)})).await {
                Ok(_) => i += 1,
                Err(e) => break e,
            }
            assert!(i < 10);
        };
        assert!(err.contains("blob store nearly full") && err.contains("RPC puts stop at 120000"), "{err}");
        let s = api.call("getBlobStats", json!({})).await;
        assert_eq!(s["puts_open"], json!(false), "{s}");
        // the loose uploads are prunable after the grace period; the segments stay
        api.mine_n(GRACE + 12).await;
        assert!(!api.has(&loose).await);
        let s = api.call("getBlobStats", json!({})).await;
        assert_eq!(s["classes"]["other"]["count"], json!(0), "{s}");
        assert_eq!(s["classes"]["segment_a"]["count"], json!(2));
        assert!(api.has(&segs[0].1).await && api.has(&segs[1].1).await);
        assert_eq!(s["puts_open"], json!(true), "room again after the prune");
    });
}

/// Reorgs: (1) a segment mined in a block that is not final yet keeps its A on a mining-only
/// node, so after a reorg to a branch where it is unmined the node offers it again; (2) once the
/// mined block is final the A is pruned on every mining-only node, and a later reorg above the
/// finalized block cannot make the segment mineable again (no fetch is wanted). (3) A fresh
/// node syncing after the pruning gets the whole chain (consensus never reads A), the pinned
/// model manifest, and the A of a still-mineable segment, and never wants the pruned A.
#[test]
fn reorg_around_mined_and_pruned_segments_and_fresh_node_after_pruning() {
    runner(43).start(|ctx| async move {
        let cfg = |_| blobs::BlobConfig { grace_blocks: 5, segment_mine_blocks: 0, keep_replay: false, ..Default::default() };
        let net = setup(&ctx, 3, 20, cfg).await;
        net.link(0, 1).await;
        ctx.sleep(Duration::from_secs(1)).await;
        let (a, b) = (&net.apis[0], &net.apis[1]);
        let m = fund_and_model(a).await;
        let tid = post_ticket(a, &m, 0, 1000, 4).await;
        let segs = post_round(a, &m, tid, 0, 0, 1).await;
        let (s0, a0) = segs[0].clone();
        a.mine_n(12).await; // past the by-age grace: only the rule keeps A now
        wait_tips_equal(&ctx, &net.apis[..2], 20).await;
        assert!(wait_blob(&ctx, b, &a0, 20).await);
        // (1) partition; node 0 mines segment 0 then 11 fallbacks (a prune pass runs), node 1 a
        // heavier branch without it
        net.unlink(0, 1).await;
        ctx.sleep(Duration::from_secs(1)).await;
        let fork = a.height().await;
        a.mine_with(Some(&s0)).await;
        a.mine_n(11).await;
        assert!(a.has(&a0).await, "mined but not final (F is below the block): A kept");
        b.mine_n(14).await;
        net.link(0, 1).await;
        let (_, h) = wait_tips_equal(&ctx, &net.apis[..2], 30).await;
        assert_eq!(h, fork + 14, "node 0 reorged to node 1's branch");
        let j = a.call("getJob", json!({"job_id": s0})).await;
        assert_eq!(j["blocks_mined"], json!(0), "segment 0 is unmined again");
        assert!(a.has(&a0).await, "A survived the reorg");
        let t = a.call("getBlockTemplate", json!({"miner": bs58::encode(pk(9)).into_string(), "job_id": s0})).await;
        assert_eq!(t["job"]["job_id"], json!(s0), "and node 0 can mine it again");
        // (2) mine it for good and bury it past finality: pruned on both nodes
        a.mine_with(Some(&s0)).await;
        let mined = a.height().await;
        let gone = mine_until_gone(a, &a0, 60).await;
        assert!(gone > mined + 20, "pruned only once final ({gone}, mined {mined})");
        wait_tips_equal(&ctx, &net.apis[..2], 30).await;
        let _ = mine_until_gone(b, &a0, 30).await;
        // a reorg above F: both sides mine a short branch, node 1's wins
        net.unlink(0, 1).await;
        ctx.sleep(Duration::from_secs(1)).await;
        a.mine_n(2).await;
        b.mine_n(4).await;
        net.link(0, 1).await;
        wait_tips_equal(&ctx, &net.apis[..2], 30).await;
        a.mine_n(11).await; // a prune pass after the reorg
        assert!(!a.has(&a0).await && !a.fetching(&a0).await, "a final mined segment's A is not wanted back after a reorg");
        // a second round (still mineable) for the fresh node to fetch
        let segs2 = post_round(a, &m, tid, 1, 1, 9).await;
        a.mine_n(11).await;
        // (3) node 2 joins fresh
        net.link(2, 0).await;
        net.link(2, 1).await;
        let (tip, _) = wait_tips_equal(&ctx, &net.apis, 60).await;
        let (i0, i2) = (a.info().await, net.apis[2].info().await);
        assert_eq!(i0["finalized_hash"], i2["finalized_hash"]);
        assert_eq!(i2["tip"], json!(tip));
        assert!(wait_blob(&ctx, &net.apis[2], &m.manifest, 30).await, "the pinned model manifest is fetched");
        // the safety-net fetch runs on a prune pass: give the fresh node a few
        for _ in 0..12 {
            a.mine().await;
        }
        assert!(wait_blob(&ctx, &net.apis[2], &segs2[0].1, 60).await, "A of a mineable segment is fetched");
        assert!(!net.apis[2].has(&a0).await && !net.apis[2].fetching(&a0).await, "the pruned A is never wanted");
        let j2 = net.apis[2].call("getJob", json!({"job_id": s0})).await;
        assert_eq!(j2["blocks_mined"], json!(1), "the fresh node has the mined segment's state");
    });
}
