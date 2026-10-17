//! Pool against the in-process RewardNode (pearl-node MockNode + coinbase maturity +
//! Transfers). No sockets: HTTP routes are exercised through `http::route`.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use base64::Engine as _;
use ed25519_dalek::SigningKey;
use serde_json::{json, Value};

use pearl_job::job::nbits_to_target;
use pearl_job::pow::{block_proof_to_bytes, fallback_job, BlockHeader};
use pearl_node::b58;
use pearl_pool::client::{PoolApi, PoolWork};
use pearl_pool::testing::RewardNode;
use pearl_pool::{pplns_split, share_bits_for, target_to_nbits, work_of, BlockStatus, PayoutStatus, Pool, PoolConfig, ShareOutcome, ShareRec};

const CHAIN: u32 = 4242;
/// About 0.6 wins per nonce on 64 x 64 fallback matrices.
const EASY: u32 = 0x1e040000;
/// No block in any test's lifetime.
const HARD: u32 = 0x1b010000;
const K: u64 = 3;

fn pool_key() -> SigningKey {
    SigningKey::from_bytes(&[0x50; 32])
}
fn miner(i: u8) -> [u8; 32] {
    SigningKey::from_bytes(&[0x60 + i; 32]).verifying_key().to_bytes()
}

fn tmp(tag: &str) -> PathBuf {
    let d = std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("pool-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d.join("pool.json")
}

fn cfg(state: PathBuf) -> PoolConfig {
    PoolConfig {
        fee_bps: 250,
        pplns_n: 6,
        maturity: None,
        share_bits: Some(EASY),
        min_payout: 1,
        payout_fee: Some(1000),
        template_refresh: Duration::from_secs(3600),
        resend_after: Duration::from_secs(3600),
        state_path: state,
        ..PoolConfig::default()
    }
}

type Log = Arc<Mutex<Vec<Value>>>;

fn open(node: &RewardNode, c: PoolConfig) -> (Pool, Log) {
    let log: Log = Arc::default();
    let l2 = log.clone();
    let p = Pool::open(Arc::new(node.clone()), pool_key(), c, Box::new(move |v| l2.lock().unwrap().push(v))).unwrap();
    (p, log)
}

/// First share (by nonce) from `from` on this work, mined at `bits` on `header`.
fn find_share(header: &BlockHeader, w: &PoolWork, bits: u32, from: u64) -> (u64, Vec<u8>) {
    for n in from..from + 200 {
        let mut h = *header;
        h.nonce = n;
        let pd = h.pow_digest();
        let fj = fallback_job(&pd, &w.template.fallback).unwrap();
        if let Some(p) = fj.mine_block(&pd, bits, n) {
            return (n, block_proof_to_bytes(&p));
        }
    }
    panic!("no share in 200 nonces")
}

fn share(pool: &Pool, m: &[u8; 32], w: &PoolWork, from: u64) -> (u64, ShareOutcome) {
    let (n, p) = find_share(&w.template.header, w, w.share_bits, from);
    (n, PoolApi::submit_share(pool, m, &w.work_id, n, &p).unwrap())
}

fn rejected(o: &ShareOutcome) -> String {
    match o {
        ShareOutcome::Rejected { reason, .. } => reason.clone(),
        other => panic!("expected a rejection, got {other:?}"),
    }
}

#[test]
fn compact_targets_and_share_bits() {
    for bits in [0x207fffffu32, 0x1e040000, 0x1d00ffff, 0x1b010000, 0x03123456] {
        assert_eq!(target_to_nbits(nbits_to_target(bits)), bits, "{bits:#x}");
    }
    // Share target never harder than the block target, never easier than the pow limit.
    assert_eq!(share_bits_for(0x1e040000, Some(0x1e010000), 16), 0x1e040000);
    assert_eq!(share_bits_for(0x1e010000, Some(0x1e040000), 16), 0x1e040000);
    assert_eq!(share_bits_for(0x207fffff, None, 16), 0x207fffff);
    let s = share_bits_for(0x1e010000, None, 16);
    assert_eq!(nbits_to_target(s), nbits_to_target(0x1e010000) * 16);
    assert!(work_of(0x1e010000) > work_of(s));
    assert_eq!(work_of(0x1e010000) / work_of(s), 16);
}

fn sr(seq: u64, miner: u32, weight: u128) -> ShareRec {
    ShareRec { seq, miner, weight, height: 1 }
}

#[test]
fn pplns_math_is_exact() {
    // 3 miners, weights 1:2:3 (6 shares of weight 1), reward 1_000_001, fee 2.5%.
    let w: Vec<ShareRec> = [1, 2, 2, 3, 3, 3].iter().enumerate().map(|(i, m)| sr(i as u64, *m, 1)).collect();
    let s = pplns_split(1_000_001, 250, &w);
    assert_eq!(s.fee, 25_000); // floor(1_000_001 * 250 / 10000)
    let net = 1_000_001 - 25_000;
    assert_eq!(s.credits[&1], net / 6);
    assert_eq!(s.credits[&2], net * 2 / 6);
    assert_eq!(s.credits[&3], net * 3 / 6);
    assert_eq!(s.credits.values().sum::<u64>() + s.fee + s.remainder, 1_000_001);
    assert_eq!(s.remainder, net - net / 6 - net * 2 / 6 - net * 3 / 6);
    assert!(s.remainder < 3);
    // Weighted shares count by work, not by number.
    let s = pplns_split(900, 0, &[sr(1, 1, 1), sr(2, 2, 2)]);
    assert_eq!((s.credits[&1], s.credits[&2], s.remainder), (300, 600, 0));
    // Edge cases: no shares (all to the pool), 100% fee, zero reward, huge weights.
    let s = pplns_split(777, 100, &[]);
    assert!(s.credits.is_empty());
    assert_eq!(s.fee + s.remainder, 777);
    let s = pplns_split(777, 10_000, &[sr(1, 1, 1)]);
    assert_eq!((s.fee, s.remainder), (777, 0));
    assert!(s.credits.is_empty());
    assert_eq!(pplns_split(0, 100, &[sr(1, 1, 5)]).credits.len(), 0);
    let s = pplns_split(u64::MAX, 1, &[sr(1, 1, u128::MAX / 3), sr(2, 2, u128::MAX / 3)]);
    assert_eq!(s.credits.values().sum::<u64>() + s.fee + s.remainder, u64::MAX);
    // Many random windows: the identity always holds and every miner gets floor(net * w / W).
    let mut x: u64 = 0x1234_5678;
    for _ in 0..500 {
        x = x.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        let reward = x >> 20;
        let bps = (x % 2000) as u32;
        let win: Vec<ShareRec> = (0..(x % 50 + 1)).map(|i| sr(i, ((x >> (i % 32)) % 7) as u32 + 1, (i as u128 % 5) + 1)).collect();
        let s = pplns_split(reward, bps, &win);
        assert_eq!(s.credits.values().sum::<u64>() + s.fee + s.remainder, reward);
        let total: u128 = win.iter().map(|r| r.weight).sum();
        let net = (reward - s.fee) as u128;
        for (m, c) in &s.credits {
            let wi: u128 = win.iter().filter(|r| r.miner == *m).map(|r| r.weight).sum();
            assert_eq!(*c as u128, net * wi / total);
        }
        assert!((s.remainder as u128) < win.len() as u128 + 1);
    }
}

#[test]
fn share_accept_and_reject_rules() {
    let node = RewardNode::new(CHAIN, HARD, K, 0);
    let (pool, _log) = open(&node, cfg(tmp("rules")));
    let (a, b) = (miner(1), miner(2));
    let wa = PoolApi::work(&pool, &a, false).unwrap();
    let wb = PoolApi::work(&pool, &b, false).unwrap();
    assert_eq!(wa.work_id, wb.work_id, "same template for everyone");
    assert_eq!(wa.template.header.miner, pool.pool_key(), "header pays the pool");
    assert!(wa.nonce_end < wb.nonce_start, "disjoint nonce ranges");
    assert_eq!(wa.share_bits, EASY);

    // Accepted share; not a block (HARD target).
    let (n1, o) = share(&pool, &a, &wa, wa.nonce_start);
    assert_eq!(o, ShareOutcome::Accepted { block: None });

    // Duplicate (same work, same nonce).
    let (_, p1) = find_share(&wa.template.header, &wa, EASY, n1);
    assert_eq!(rejected(&PoolApi::submit_share(&pool, &a, &wa.work_id, n1, &p1).unwrap()), "duplicate share");

    // Nonce from another miner's range (b replays a's share).
    assert!(rejected(&PoolApi::submit_share(&pool, &b, &wa.work_id, n1, &p1).unwrap()).contains("outside this miner's range"));

    // Low work: a proof that only meets an easier target than the pool's share target.
    let hard_share = Pool::open(
        Arc::new(node.clone()),
        SigningKey::from_bytes(&[0x51; 32]),
        PoolConfig { share_bits: Some(0x1d010000), ..cfg(tmp("lowwork")) },
        Box::new(|_| {}),
    )
    .unwrap();
    let wl = PoolApi::work(&hard_share, &a, false).unwrap();
    let mut low = None;
    for n in wl.nonce_start..wl.nonce_start + 200 {
        let (m, p) = find_share(&wl.template.header, &wl, 0x207fffff, n);
        if let ShareOutcome::Rejected { reason, .. } = PoolApi::submit_share(&hard_share, &a, &wl.work_id, m, &p).unwrap() {
            low = Some(reason);
            break;
        }
    }
    assert!(low.expect("some proof misses 0x1d010000").contains("low work"));

    // Wrong template: mined on a header paying the miner itself, sent as pool work.
    let mut own = wa.template.header;
    own.miner = a;
    let (n2, p2) = find_share(&own, &wa, EASY, n1 + 1);
    assert!(rejected(&PoolApi::submit_share(&pool, &a, &wa.work_id, n2, &p2).unwrap()).contains("low work or invalid"));

    // Unknown work id, garbage proof, unregistered miner.
    assert!(rejected(&PoolApi::submit_share(&pool, &a, "00ff", n2, &p2).unwrap()).contains("unknown"));
    assert!(rejected(&PoolApi::submit_share(&pool, &a, &wa.work_id, n2, b"junk").unwrap()).contains("invalid proof"));
    assert_eq!(rejected(&PoolApi::submit_share(&pool, &miner(9), &wa.work_id, n2, &p2).unwrap()), "unregistered miner");

    // Stale after a new block: the tip moves, the old work is refused.
    let (n3, p3) = find_share(&wa.template.header, &wa, EASY, n1 + 1);
    node.advance(1);
    pool.refresh().unwrap();
    match PoolApi::submit_share(&pool, &a, &wa.work_id, n3, &p3).unwrap() {
        ShareOutcome::Rejected { stale: true, .. } => {}
        o => panic!("expected stale, got {o:?}"),
    }
    let w2 = PoolApi::work(&pool, &a, false).unwrap();
    assert_ne!(w2.work_id, wa.work_id);
    assert_eq!(w2.template.header.height, wa.template.header.height + 1);
    assert_eq!(share(&pool, &a, &w2, w2.nonce_start).1, ShareOutcome::Accepted { block: None });

    let st = pool.state();
    assert_eq!(st.totals.shares, 2);
    assert_eq!(st.window.len(), 2);
    assert_eq!(st.miners[&b58(&a)].shares, 2);
    assert_eq!(st.miners[&b58(&a)].rejected, 5); // duplicate, wrong template, unknown, junk, stale
    assert_eq!(st.miners[&b58(&b)].rejected, 1);
    assert!(st.blocks.is_empty());
}

#[test]
fn block_found_by_share_then_pplns_payout_after_maturity() {
    let reward = 1_000_000_007u64;
    let node = RewardNode::new(CHAIN, HARD, K, reward);
    let (pool, _log) = open(&node, cfg(tmp("payout")));
    assert_eq!(pool.maturity(), K + 1);
    let (a, b) = (miner(1), miner(2));
    let wa = PoolApi::work(&pool, &a, false).unwrap();
    let wb = PoolApi::work(&pool, &b, false).unwrap();
    // a: 1 share, b: 2 shares (none is a block at HARD).
    let (na, _) = share(&pool, &a, &wa, wa.nonce_start);
    let (nb, _) = share(&pool, &b, &wb, wb.nonce_start);
    share(&pool, &b, &wb, nb + 1);
    let _ = na;
    // Make the block target the share target: a's next share is a block.
    node.set_target(EASY);
    node.advance(1); // new tip so the new target applies
    pool.refresh().unwrap();
    let wa = PoolApi::work(&pool, &a, false).unwrap();
    assert_eq!(wa.template.header.target_bits, EASY);
    let h0 = node.mock.height();
    let (_, o) = share(&pool, &a, &wa, wa.nonce_start + 100);
    let ShareOutcome::Accepted { block: Some(blk) } = o else { panic!("expected a block: {o:?}") };
    assert!(blk.accepted, "{blk:?}");
    assert_eq!(node.mock.height(), h0 + 1);
    let st = pool.state();
    assert_eq!(st.blocks.len(), 1);
    let fb = &st.blocks[0];
    assert_eq!(fb.status, BlockStatus::Immature);
    assert_eq!(fb.reward, reward);
    // Window: a 2 shares (incl. the block share), b 2 shares; equal weight.
    let fee = reward * 250 / 10_000;
    let net = reward - fee;
    let (ia, ib) = (st.miners[&b58(&a)].id, st.miners[&b58(&b)].id);
    assert_eq!(fb.fee, fee);
    assert_eq!(fb.credits[&ia], net * 2 / 4);
    assert_eq!(fb.credits[&ib], net * 2 / 4);
    assert_eq!(fb.credits.values().sum::<u64>() + fb.fee + fb.remainder, reward);
    assert_eq!(st.miners[&b58(&a)].immature, net / 2);
    assert_eq!(st.miners[&b58(&a)].blocks, 1);

    // Not mature yet.
    node.advance(K - 1);
    pool.tick().unwrap();
    assert_eq!(pool.state().blocks[0].status, BlockStatus::Immature);
    assert_eq!(node.balance(&pool.pool_key()), 0);
    // Chain credits at +K; the pool waits one more (maturity K + 1).
    node.advance(1);
    assert_eq!(node.balance(&pool.pool_key()), reward);
    pool.tick().unwrap();
    assert_eq!(pool.state().blocks[0].status, BlockStatus::Immature);
    node.advance(1);
    pool.tick().unwrap();
    let st = pool.state();
    assert_eq!(st.blocks[0].status, BlockStatus::Matured);
    assert_eq!(st.totals.rewards_matured, reward);
    assert_eq!(st.totals.pool_earned, fee + fb.remainder);
    // One batched round: both payouts signed with consecutive nonces, pending.
    let pend: Vec<_> = st.payouts.iter().filter(|p| p.status == PayoutStatus::Pending).collect();
    assert_eq!(pend.len(), 2);
    assert_eq!(pend.iter().map(|p| p.nonce).collect::<Vec<_>>(), vec![0, 1]);
    assert_eq!(st.miners[&b58(&a)].balance, 0);
    assert_eq!(st.miners[&b58(&a)].in_flight, net / 2);
    assert_eq!(node.mempool_len(), 2);
    // Next block includes them; the pool confirms by the account nonce.
    node.advance(1);
    pool.tick().unwrap();
    let st = pool.state();
    assert!(st.payouts.iter().all(|p| p.status == PayoutStatus::Confirmed));
    for (m, id) in [(a, ia), (b, ib)] {
        let r = &st.miners[&b58(&m)];
        assert_eq!(r.paid, fb.credits[&id] - 1000, "payout = credit - tx fee");
        assert_eq!(r.tx_fees, 1000);
        assert_eq!((r.balance, r.in_flight, r.immature), (0, 0, 0));
        assert_eq!(node.balance(&m), fb.credits[&id] - 1000);
    }
    // Exact lamport accounting: what is left on the pool key is the pool's cut.
    assert_eq!(st.totals.paid_out, fb.credits.values().sum::<u64>());
    assert_eq!(node.balance(&pool.pool_key()), st.totals.rewards_matured - st.totals.paid_out);
    assert_eq!(node.balance(&pool.pool_key()), fee + fb.remainder);
    // Nothing more to pay.
    pool.tick().unwrap();
    assert_eq!(pool.state().payouts.len(), 2);
}

#[test]
fn orphaned_block_drops_its_credits() {
    let node = RewardNode::new(CHAIN, EASY, K, 5_000_000);
    let (pool, _) = open(&node, cfg(tmp("orphan")));
    let a = miner(1);
    let w = PoolApi::work(&pool, &a, false).unwrap();
    let (_, o) = share(&pool, &a, &w, w.nonce_start);
    let ShareOutcome::Accepted { block: Some(b) } = o else { panic!("{o:?}") };
    assert!(b.accepted);
    assert!(pool.state().miners[&b58(&a)].immature > 0);
    // Our own block moves the tip at once: another share on the same work is stale (no
    // sibling block is submitted).
    let (n2, p2) = find_share(&w.template.header, &w, EASY, w.nonce_start + 1);
    assert!(matches!(PoolApi::submit_share(&pool, &a, &w.work_id, n2, &p2).unwrap(), ShareOutcome::Rejected { stale: true, .. }));
    assert_eq!(node.mock.height(), b.height);
    node.reorg_at(b.height);
    node.advance(K + 1);
    pool.tick().unwrap();
    let st = pool.state();
    assert_eq!(st.blocks[0].status, BlockStatus::Orphaned);
    assert_eq!(st.totals.blocks_orphaned, 1);
    let r = &st.miners[&b58(&a)];
    assert_eq!((r.immature, r.balance, r.in_flight), (0, 0, 0));
    assert!(st.payouts.is_empty());
    assert_eq!(node.balance(&pool.pool_key()), 0);
}

#[test]
fn restart_keeps_shares_dedup_and_pending_payouts() {
    let reward = 2_000_000_000u64;
    let state = tmp("restart");
    let node = RewardNode::new(CHAIN, HARD, K, reward);
    let (a, b) = (miner(1), miner(2));
    let (w, n1, p1);
    {
        let (pool, _) = open(&node, cfg(state.clone()));
        w = PoolApi::work(&pool, &a, false).unwrap();
        let wb = PoolApi::work(&pool, &b, false).unwrap();
        let (n, p) = find_share(&w.template.header, &w, EASY, w.nonce_start);
        (n1, p1) = (n, p.clone());
        assert_eq!(PoolApi::submit_share(&pool, &a, &w.work_id, n, &p).unwrap(), ShareOutcome::Accepted { block: None });
        share(&pool, &b, &wb, wb.nonce_start);
    }
    // Restart 1: same template -> same work id; the duplicate is still refused.
    let (pool, _) = open(&node, cfg(state.clone()));
    let st = pool.state();
    assert_eq!(st.window.len(), 2);
    assert_eq!(st.miners.len(), 2);
    let w2 = PoolApi::work(&pool, &a, false).unwrap();
    assert_eq!(w2.work_id, w.work_id);
    assert_eq!(w2.nonce_start, w.nonce_start, "registration survives");
    assert_eq!(rejected(&PoolApi::submit_share(&pool, &a, &w.work_id, n1, &p1).unwrap()), "duplicate share");
    // A block, matured, payouts signed and sent, then a crash before they land.
    node.set_target(EASY);
    node.advance(1);
    pool.refresh().unwrap();
    let w3 = PoolApi::work(&pool, &a, false).unwrap();
    let (_, o) = share(&pool, &a, &w3, w3.nonce_start + 50);
    assert!(matches!(o, ShareOutcome::Accepted { block: Some(ref b) } if b.accepted), "{o:?}");
    node.set_target(HARD);
    node.advance(K + 1);
    pool.tick().unwrap();
    let before = pool.state();
    let pend: Vec<_> = before.payouts.iter().filter(|p| p.status == PayoutStatus::Pending).cloned().collect();
    assert_eq!(pend.len(), 2);
    drop(pool);
    assert_eq!(node.mempool_len(), 2);
    let (pool, _) = open(&node, PoolConfig { resend_after: Duration::ZERO, ..cfg(state.clone()) });
    let after = pool.state();
    assert_eq!(after.payouts, before.payouts, "pending payouts reloaded as signed");
    assert_eq!(after.next_nonce, before.next_nonce);
    pool.tick().unwrap();
    // Re-send of the same bytes is refused by the node (already pending); no new nonce.
    let st = pool.state();
    assert_eq!(st.payouts.len(), 2);
    assert_eq!(st.next_nonce, before.next_nonce);
    node.advance(1);
    pool.tick().unwrap();
    let st = pool.state();
    assert!(st.payouts.iter().all(|p| p.status == PayoutStatus::Confirmed));
    let total_paid: u64 = st.payouts.iter().map(|p| p.amount + p.fee).sum();
    assert_eq!(node.balance(&pool.pool_key()), reward - total_paid);
    assert_eq!(st.totals.paid_out, total_paid);
    // Restart 2: nothing is paid twice.
    drop(pool);
    let (pool, _) = open(&node, cfg(state));
    node.advance(2);
    pool.tick().unwrap();
    assert_eq!(pool.state().payouts.len(), 2);
    assert_eq!(node.balance(&pool.pool_key()), reward - total_paid);
}

#[test]
fn restart_resends_payout_the_node_dropped() {
    let reward = 3_000_000u64;
    let state = tmp("resend");
    let node = RewardNode::new(CHAIN, EASY, K, reward);
    let a = miner(1);
    let (pool, _) = open(&node, cfg(state.clone()));
    let w = PoolApi::work(&pool, &a, false).unwrap();
    assert!(matches!(share(&pool, &a, &w, w.nonce_start).1, ShareOutcome::Accepted { block: Some(_) }));
    node.set_target(HARD);
    node.advance(K + 1);
    pool.tick().unwrap();
    assert_eq!(node.mempool_len(), 1);
    let sent = pool.state().payouts[0].clone();
    assert_eq!(sent.sends, 1);
    drop(pool);
    // Pool and node both restart; the node forgot the tx.
    node.drop_mempool();
    let (pool, _) = open(&node, PoolConfig { resend_after: Duration::ZERO, ..cfg(state.clone()) });
    pool.tick().unwrap();
    assert_eq!(node.mempool_len(), 1, "re-sent");
    let st = pool.state();
    assert_eq!(st.payouts.len(), 1, "no second payout");
    assert_eq!(st.payouts[0].tx, sent.tx, "same signed bytes, same nonce");
    assert_eq!(st.payouts[0].sends, 2);
    node.advance(1);
    pool.tick().unwrap();
    let st = pool.state();
    assert_eq!(st.payouts[0].status, PayoutStatus::Confirmed);
    let wire = base64::engine::general_purpose::STANDARD.decode(&sent.tx).unwrap();
    let (tx, _) = pearl_node::tx::decode(&wire).unwrap();
    assert_eq!((tx.nonce, tx.from, tx.fee), (0, pool_key().verifying_key().to_bytes(), 1000));
    assert!(matches!(tx.body, pearl_node::tx::Body::Transfer { to, amount } if to == a && amount == sent.amount));
    assert_eq!(node.balance(&a), sent.amount);
    assert_eq!(node.balance(&pool.pool_key()), reward - sent.amount - sent.fee);
}

#[test]
fn http_routes() {
    let node = RewardNode::new(CHAIN, HARD, K, 0);
    let (pool, _) = open(&node, cfg(tmp("http")));
    let a = miner(3);
    let (c, v) = pearl_pool::http::route(&pool, "GET", "/health", b"");
    assert_eq!((c, v), (200, json!("ok")));
    let (c, reg) = pearl_pool::http::route(&pool, "POST", "/register", json!({"payout": b58(&a)}).to_string().as_bytes());
    assert_eq!(c, 200);
    assert_eq!(reg["id"], 1);
    let (c, wv) = pearl_pool::http::route(&pool, "POST", "/work", json!({"payout": b58(&a)}).to_string().as_bytes());
    assert_eq!(c, 200, "{wv}");
    let w = PoolWork::from_json(&wv).unwrap();
    let (n, p) = find_share(&w.template.header, &w, w.share_bits, w.nonce_start);
    let body = json!({"payout": b58(&a), "work_id": w.work_id, "nonce": n, "proof": base64::engine::general_purpose::STANDARD.encode(&p)});
    let (c, r) = pearl_pool::http::route(&pool, "POST", "/share", body.to_string().as_bytes());
    assert_eq!(c, 200);
    assert_eq!(r["accepted"], true);
    assert_eq!(pearl_pool::client::share_from_json(&r).unwrap(), ShareOutcome::Accepted { block: None });
    let (c, r) = pearl_pool::http::route(&pool, "POST", "/share", body.to_string().as_bytes());
    assert_eq!((c, r["accepted"].clone(), r["reason"].clone()), (200, json!(false), json!("duplicate share")));
    let (_, s) = pearl_pool::http::route(&pool, "GET", "/stats", b"");
    assert_eq!(s["totals"]["shares"], 1);
    assert_eq!(s["share_bits"], EASY);
    let (_, m) = pearl_pool::http::route(&pool, "GET", &format!("/miner?payout={}", b58(&a)), b"");
    assert_eq!(m["miner"]["shares"], 1);
    assert_eq!(m["window_shares"], 1);
    let (c, _) = pearl_pool::http::route(&pool, "GET", "/nope", b"");
    assert_eq!(c, 404);
    let (c, e) = pearl_pool::http::route(&pool, "POST", "/share", b"{}");
    assert_eq!(c, 400, "{e}");
}
