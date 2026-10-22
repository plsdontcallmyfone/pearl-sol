//! Pre-validation buffers under flooding (security pass 2026-10-21).
//!
//! Orphans (blocks whose parent is unknown) and header-only entries cannot have a job
//! block's work checked yet, so a peer can send them for free. Before the fix the buffers
//! held up to 4096 blocks of up to 16 MiB and 16384 proofs of up to 4 MiB, never pruned at
//! far heights, and once full refused every honest orphan or header ("orphan pool full",
//! "header buffer full"). Now they have byte budgets and drop their oldest entries.

use agent_types::tx::pubkey_from_seed;
use agent_types::*;
use commonware_runtime::{deterministic, Runner as _, Supervisor as _};
use consensus_pow::{Accept, Chain, Genesis, JobChoice, MockVerifier, MAX_ORPHAN_BYTES};
use state::config::Config;

const CHAIN: u32 = 7;
const EASY: u32 = 0x207fffff;
const T0: u64 = 1_760_000_000_000;

type C = Chain<deterministic::Context, MockVerifier>;

fn genesis() -> Genesis {
    let mut c = Config::defaults(pubkey_from_seed(&[1; 32]));
    c.finality_k = 5;
    c.target_block_time_ms = 1000;
    c.attesters = vec![pubkey_from_seed(&[50; 32])];
    Genesis { chain_id: CHAIN, timestamp_ms: T0, target_bits: EASY, config: c }
}

/// A self-consistent block (commitments match) on an unknown parent: free to make.
fn junk_orphan(i: u64, height: u64, proof_len: usize) -> Block {
    let miner = [7; 32];
    let cb = Coinbase { height, miner, fees: 0, subsidy: 0, job_id: [0; 32], mature_height: height + 5 };
    let proof = {
        let mut p = vec![0u8; proof_len];
        p[..8].copy_from_slice(&i.to_le_bytes());
        p
    };
    let mut prev = [0xee; 32];
    prev[..8].copy_from_slice(&i.to_le_bytes());
    let header = Header {
        version: 1,
        height,
        prev_hash: prev,
        timestamp_ms: T0 + 1000,
        target_bits: EASY,
        miner,
        tx_root: block::tx_root_with(&[], Some(&cb)),
        proof_hash: block::proof_hash(&proof),
        ..Default::default()
    };
    Block { header, proof, txs: vec![], coinbase: Some(cb), contracts: None, uncles: Vec::new() }
}

#[test]
fn orphan_flood_by_count_keeps_taking_honest_orphans() {
    deterministic::Runner::default().start(|ctx| async move {
        let mut c = C::open(ctx.child("a"), "n", genesis(), MockVerifier).await.unwrap();
        let g = c.genesis_hash();
        // 5000 junk orphans at a far height (never pruned by finality)
        for i in 0..5000u64 {
            let r = c.insert_block(junk_orphan(i, 1_000_000, 16), T0 + 1000).await;
            assert!(matches!(r, Ok((Accept::Orphan { .. }, _))), "junk orphan {i}: {r:?}");
        }
        assert!(c.orphan_count() <= 4096);
        // an honest block and its child arrive child first: the child is an orphan and is kept
        let t1 = c.build_template_at(g, [3; 32], JobChoice::Fallback, &[], T0 + 1000).await.unwrap();
        let b1 = seal(&t1);
        // build b2 on b1 in a scratch chain to get a valid child
        let mut scratch = C::open(ctx.child("s"), "s", genesis(), MockVerifier).await.unwrap();
        scratch.insert_block(b1.clone(), T0 + 1000).await.unwrap();
        let t2 = scratch.build_template_at(b1.hash(), [3; 32], JobChoice::Fallback, &[], T0 + 2000).await.unwrap();
        let b2 = seal(&t2);
        let r = c.insert_block(b2.clone(), T0 + 2000).await.unwrap();
        assert!(matches!(r.0, Accept::Orphan { .. }), "honest orphan refused: {r:?}");
        let r = c.insert_block(b1, T0 + 2000).await.unwrap();
        assert_eq!(r.0, Accept::Added { tip: true });
        assert_eq!(c.tip().hash, b2.hash(), "the honest orphan connected");
    });
}

#[test]
fn orphan_flood_by_bytes_is_bounded() {
    deterministic::Runner::default().start(|ctx| async move {
        let mut c = C::open(ctx.child("a"), "n", genesis(), MockVerifier).await.unwrap();
        let big = agent_types::block::MAX_PROOF_LEN;
        for i in 0..80u64 {
            c.insert_block(junk_orphan(i, 1_000_000, big), T0 + 1000).await.unwrap();
        }
        let ob = c.orphan_bytes();
        assert!(ob <= MAX_ORPHAN_BYTES, "orphan bytes {ob}");
        assert!(c.orphan_count() < 80);
    });
}

#[test]
fn header_flood_is_bounded() {
    deterministic::Runner::default().start(|ctx| async move {
        let mut c = C::open(ctx.child("a"), "n", genesis(), MockVerifier).await.unwrap();
        // job headers above unknown ancestry: their work cannot be checked yet
        for i in 0..(1u64 << 18) + 1000 {
            let b = junk_orphan(i, 1_000_000, 16);
            let mut h = b.header;
            h.job_id = [0x42; 32];
            let r = c.insert_header(h, b.proof).await;
            assert!(r.is_ok(), "header {i}: {r:?}");
        }
        assert!(c.headers_only_count() <= 1 << 18);
    });
}

fn seal(t: &consensus_pow::Template) -> Block {
    let mut h = t.header;
    let proof = MockVerifier::proof_for(&h);
    h.proof_hash = block::proof_hash(&proof);
    Block { header: h, proof, txs: t.txs.clone(), coinbase: Some(t.summary.coinbase), contracts: t.summary.contracts, uncles: Vec::new() }
}

/// A block a little too far in the future is refused now and accepted once the clock
/// catches up (before: cached as invalid for good, so a node whose clock ran behind stayed
/// on its own fork).
#[test]
fn future_block_is_retried_not_cached() {
    deterministic::Runner::default().start(|ctx| async move {
        let mut c = C::open(ctx.child("a"), "n", genesis(), MockVerifier).await.unwrap();
        let g = c.genesis_hash();
        let max_future = c.tip().config.max_future_ms;
        let now = T0 + 10_000;
        let t = c.build_template_at(g, [3; 32], JobChoice::Fallback, &[], now + max_future + 5_000).await.unwrap();
        let b = seal(&t);
        let e = c.insert_block(b.clone(), now).await.unwrap_err();
        assert!(consensus_pow::is_time_too_new(&e.to_string()), "{e}");
        let r = c.insert_block(b.clone(), now + 6_000).await.unwrap();
        assert_eq!(r.0, Accept::Added { tip: true });
        assert_eq!(c.tip().hash, b.hash());
    });
}

/// A job header on a live parent is checked against the parent's state: one naming a job the
/// parent does not have is invalid (before: stored unchecked, so anyone could hang free
/// headers off any live block and fill the body schedule with them).
#[test]
fn job_header_on_live_parent_needs_its_job() {
    deterministic::Runner::default().start(|ctx| async move {
        let mut c = C::open(ctx.child("a"), "n", genesis(), MockVerifier).await.unwrap();
        let g = c.genesis_hash();
        let t = c.build_template_at(g, [3; 32], JobChoice::Fallback, &[], T0 + 1000).await.unwrap();
        let mut h = seal(&t).header;
        h.job_id = [0x42; 32];
        let proof = MockVerifier::proof_for(&h);
        h.proof_hash = block::proof_hash(&proof);
        let e = c.insert_header(h, proof).await.unwrap_err();
        assert!(e.to_string().contains("job its parent does not have"), "{e}");
        assert_eq!(c.headers_only_count(), 0);
    });
}
