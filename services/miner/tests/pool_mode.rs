//! Pool mode: PoolMiner against an in-process pearl-pool over the pool crate's
//! RewardNode (MockNode + coinbase maturity + Transfers). No sockets.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use ed25519_dalek::SigningKey;
use serde_json::Value;

use pearl_job::pow::FallbackParams;
use pearl_job::{data, job_key, JobHeader, MiningConfig};
use pearl_miner::pool::{mine_share_nonce, PoolMiner, PoolMinerConfig};
use pearl_node::JobInfo;
use pearl_pool::testing::RewardNode;
use pearl_pool::{BlockStatus, PayoutStatus, Pool, PoolConfig};

const CHAIN: u32 = 4242;
const EASY: u32 = 0x1e040000;
/// 4x harder than EASY: most shares are not blocks.
const BLOCK: u32 = 0x1e010000;
const K: u64 = 2;

fn key(i: u8) -> [u8; 32] {
    SigningKey::from_bytes(&[0x70 + i; 32]).verifying_key().to_bytes()
}

fn tmp(tag: &str) -> std::path::PathBuf {
    let d = std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("poolmode-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn pool_on(node: &RewardNode, dir: &std::path::Path) -> Pool {
    let cfg = PoolConfig {
        fee_bps: 100,
        pplns_n: 50,
        share_bits: Some(EASY),
        min_payout: 1,
        payout_fee: Some(1000),
        template_refresh: Duration::from_secs(3600),
        state_path: dir.join("pool.json"),
        ..PoolConfig::default()
    };
    Pool::open(Arc::new(node.clone()), SigningKey::from_bytes(&[0x55; 32]), cfg, Box::new(|_| {})).unwrap()
}

fn mcfg(shares: u64) -> PoolMinerConfig {
    PoolMinerConfig { threads: 2, refresh: Duration::from_millis(200), stats_every: Duration::from_millis(1), max_shares: Some(shares) }
}

fn logger() -> (Arc<Mutex<Vec<Value>>>, pearl_miner::Logger) {
    let logs = Arc::new(Mutex::new(Vec::new()));
    let l2 = logs.clone();
    (logs, Box::new(move |v| l2.lock().unwrap().push(v)))
}

#[test]
fn share_nonce_rescans_for_a_block() {
    // At share target == block target the share is the block; at an easier share target
    // a share proof is returned for every winning nonce, and a block-meeting tile is
    // preferred when one exists.
    let node = RewardNode::new(CHAIN, BLOCK, K, 0);
    let t = pearl_node::NodeApi::fallback_template(&node, &key(0)).unwrap();
    // many tiles per proof (the 4x8 default has one), so a rescan has something to find
    let fb = FallbackParams { m: 64, n: 64, ..FallbackParams::default() };
    let (mut shares, mut blocks) = (0, 0);
    for n in 0..40 {
        if let Some(p) = mine_share_nonce(&t.header, None, &fb, EASY, n).unwrap() {
            shares += 1;
            let mut h = t.header;
            h.nonce = n;
            h.proof_hash = pearl_job::pow::proof_hash(&p);
            if pearl_job::pow::verify_block(&h, None, &p, &fb).is_ok() {
                blocks += 1;
            }
            // Every share also verifies at the share target.
            let pd = { let mut q = t.header; q.nonce = n; q.pow_digest() };
            let fj = pearl_job::pow::fallback_job(&pd, &fb).unwrap();
            let proof = pearl_job::pow::block_proof_from_bytes(&p).unwrap();
            assert!(pearl_job::pow::verify_block_proof(&fj.job, &proof, &pd, EASY, n).is_ok());
            // And the solo miner's block search at BLOCK finds a block on this nonce iff we did.
            assert_eq!(fj.mine_block(&pd, BLOCK, n).is_some(), pearl_job::pow::verify_block(&h, None, &p, &fb).is_ok());
        }
    }
    assert!(shares > blocks && blocks > 0, "shares {shares} blocks {blocks}");
}

#[test]
fn two_pool_miners_share_a_block_and_get_paid() {
    let dir = tmp("two");
    let reward = 10_000_000_000u64;
    let node = RewardNode::new(CHAIN, BLOCK, K, reward);
    let pool = pool_on(&node, &dir);
    let (a, b) = (key(1), key(2));
    let (_la, log_a) = logger();
    let (_lb, log_b) = logger();
    let mut ma = PoolMiner::new(&pool, a, mcfg(u64::MAX), log_a);
    let mut mb = PoolMiner::new(&pool, b, mcfg(u64::MAX), log_b);
    // Alternate rounds until the pool found a block with both in the window.
    let mut rounds = 0;
    while pool.state().blocks.iter().filter(|x| x.credits.len() == 2).count() == 0 {
        ma.step().unwrap();
        mb.step().unwrap();
        rounds += 1;
        assert!(rounds < 50, "no shared block after {rounds} rounds");
    }
    assert!(ma.stats.accepted > 0 && mb.stats.accepted > 0);
    let st = pool.state();
    assert!(st.blocks.iter().all(|x| x.credits.values().sum::<u64>() + x.fee + x.remainder == x.reward));
    // Blocks on the chain pay the pool key, not the miners.
    let s = node.mock.state.lock().unwrap();
    assert!(s.blocks.iter().filter(|x| x.proof_len > 0).all(|x| x.header.miner == pool.pool_key()));
    drop(s);
    // Mature everything, pay out.
    node.set_target(0x1b010000);
    node.advance(K + 2);
    pool.tick().unwrap();
    node.advance(1);
    pool.tick().unwrap();
    let st = pool.state();
    assert!(st.blocks.iter().all(|x| x.status == BlockStatus::Matured));
    assert!(st.payouts.iter().all(|p| p.status == PayoutStatus::Confirmed));
    assert!(node.balance(&a) > 0 && node.balance(&b) > 0);
    let credited: u64 = st.blocks.iter().map(|x| x.credits.values().sum::<u64>()).sum();
    assert_eq!(node.balance(&a) + node.balance(&b) + 1000 * st.payouts.len() as u64, credited);
    assert_eq!(node.balance(&pool.pool_key()), st.totals.pool_earned);
}

#[test]
fn pool_miner_mines_job_work() {
    let dir = tmp("job");
    let node = RewardNode::new(CHAIN, BLOCK, K, 1);
    let (m, n, k) = (64u32, 64u32, 1024u32);
    let (am, bm) = data::random_matrices(m as usize, n as usize, k as usize, Some(9));
    let mp = data::save(&dir.join("job"), m, n, k, &am, &bm).unwrap();
    let config = MiningConfig::localnet_default();
    let header = JobHeader { job_pubkey: [0x0c; 32], terms_hash: [0x33; 32], created_slot: 0, target_bits: 0 };
    let (ha, hb) = pearl_job::commit(&job_key(&header, &config), &am, &bm, k as usize, n as usize);
    node.add_job(JobInfo {
        job_id: [0x0c; 32],
        header_id: [0x0c; 32],
        poster: [0x10; 32],
        terms_hash: [0x33; 32],
        created_height: 0,
        share_target: 0,
        config,
        m,
        n,
        hash_a: ha,
        hash_b: hb,
        data_uri: format!("file://{}", mp.display()),
        status: "Open".into(),
        deliverer: None,
        result_hash: None,
        result_uri: String::new(),
        evaluators: vec![],
        threshold: 1,
        deadline_height: 1000,
    });
    let pool = pool_on(&node, &dir);
    let (logs, log) = logger();
    let mut pm = PoolMiner::new(&pool, key(3), mcfg(u64::MAX), log);
    let mut rounds = 0;
    while pool.state().blocks.is_empty() {
        pm.step().unwrap();
        rounds += 1;
        assert!(rounds < 50);
    }
    assert!(logs.lock().unwrap().iter().any(|v| v["event"] == "job_loaded"));
    let s = node.mock.state.lock().unwrap();
    let jb: Vec<_> = s.blocks.iter().filter(|x| x.header.job_id == [0x0c; 32]).collect();
    assert!(!jb.is_empty(), "no job block");
    assert!(jb.iter().all(|x| x.header.miner == pool.pool_key()));
}

#[test]
fn bad_job_data_falls_back_to_fallback_work() {
    let dir = tmp("badjob");
    let node = RewardNode::new(CHAIN, EASY, K, 1);
    let config = MiningConfig::localnet_default();
    node.add_job(JobInfo {
        job_id: [0x0d; 32],
        header_id: [0x0d; 32],
        poster: [0x10; 32],
        terms_hash: [0x33; 32],
        created_height: 0,
        share_target: 0,
        config,
        m: 64,
        n: 64,
        hash_a: [1; 32],
        hash_b: [2; 32],
        data_uri: "file:///nonexistent/pool-test".into(),
        status: "Open".into(),
        deliverer: None,
        result_hash: None,
        result_uri: String::new(),
        evaluators: vec![],
        threshold: 1,
        deadline_height: 1000,
    });
    let pool = pool_on(&node, &dir);
    let (logs, log) = logger();
    let mut pm = PoolMiner::new(&pool, key(4), mcfg(3), log);
    pm.run(&|| false).unwrap();
    assert!(pm.stats.accepted >= 3);
    assert!(logs.lock().unwrap().iter().any(|v| v["event"] == "job_unusable"));
    let s = node.mock.state.lock().unwrap();
    assert!(!s.blocks.is_empty() && s.blocks.iter().all(|x| x.header.job_id == [0; 32]));
}
