//! Body scheduling under header floods (C-U1).
//!
//! A header above unknown ancestry costs its sender nothing and no peer can serve a body that
//! connects it. Before the fix every stored header joined the body schedule, lowest height
//! first, and an asked body held its window slot until it arrived or finality passed it: 512
//! such headers just above the tip filled the window for good and a syncing node stalled.
//! Now only headers whose ancestry reaches a live block are scheduled, and an ask nobody
//! answers gives its slot back after `BODY_ASK_TIMEOUT_MS`.

use std::collections::HashSet;

use agent_types::tx::pubkey_from_seed;
use agent_types::*;
use commonware_runtime::{deterministic, Runner as _, Supervisor as _};
use consensus_pow::{Accept, Chain, Genesis, HeaderAccept, JobChoice, MockVerifier, BODY_ASK_TIMEOUT_MS, BODY_RETRY_SHARE, BODY_WINDOW};
use state::config::Config;

const CHAIN: u32 = 7;
const EASY: u32 = 0x207fffff;
const T0: u64 = 1_760_000_000_000;

type C = Chain<deterministic::Context, MockVerifier>;

fn genesis() -> Genesis {
    let mut c = Config::defaults(pubkey_from_seed(&[1; 32]));
    c.finality_k = 10;
    c.target_block_time_ms = 1000;
    c.attesters = vec![pubkey_from_seed(&[50; 32])];
    Genesis { chain_id: CHAIN, timestamp_ms: T0, target_bits: EASY, config: c }
}

fn seal(t: &consensus_pow::Template) -> Block {
    let mut h = t.header;
    let proof = MockVerifier::proof_for(&h);
    h.proof_hash = block::proof_hash(&proof);
    Block { header: h, proof, txs: t.txs.clone(), coinbase: Some(t.summary.coinbase), contracts: t.summary.contracts, uncles: Vec::new() }
}

/// Extend `parent` by `n` empty fallback blocks; returns them in height order.
async fn extend(c: &mut C, mut parent: Hash, n: usize, miner: u8, now: &mut u64) -> Vec<Block> {
    let mut out = Vec::new();
    for _ in 0..n {
        *now += 1000;
        let t = c.build_template_at(parent, [miner; 32], JobChoice::Fallback, &[], *now).await.unwrap();
        let b = seal(&t);
        parent = b.hash();
        c.insert_block(b.clone(), *now).await.unwrap();
        out.push(b);
    }
    out
}

/// A job header on an unknown random parent: free to make, its work cannot be checked, and no
/// peer can serve a body for it.
fn junk_header(i: u64, height: u64) -> (Header, Vec<u8>) {
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
        target_bits: EASY,
        miner: [7; 32],
        job_id: [0x42; 32],
        proof_hash: block::proof_hash(&proof),
        ..Default::default()
    };
    (header, proof)
}

/// Headers the way backfill delivers them: the tip first, each one naming its parent.
async fn backfill_headers(dst: &mut C, blocks: &[Block]) {
    for (i, b) in blocks.iter().enumerate().rev() {
        let r = dst.insert_header(b.header, b.proof.clone()).await.unwrap();
        let want = if i == 0 { None } else { Some(b.header.prev_hash) };
        assert_eq!(r, HeaderAccept::Stored { need_parent: want }, "header at height {}", b.header.height);
    }
}

/// Source chain of `shared + ahead` blocks and a node holding the first `shared` of them.
async fn pair(ctx: &deterministic::Context, shared: usize, ahead: usize, now: &mut u64) -> (C, C, Vec<Block>) {
    let mut src = C::open(ctx.child("src"), "s", genesis(), MockVerifier).await.unwrap();
    let mut dst = C::open(ctx.child("dst"), "d", genesis(), MockVerifier).await.unwrap();
    let g = src.genesis_hash();
    let prefix = extend(&mut src, g, shared, 3, now).await;
    for b in &prefix {
        dst.insert_block(b.clone(), *now).await.unwrap();
    }
    let rest = extend(&mut src, prefix.last().map(|b| b.hash()).unwrap_or(g), ahead, 3, now).await;
    (src, dst, rest)
}

/// The flood of the finding: 600 junk headers on unknown random parents, 300 of them at
/// tip + 1 and the rest at tip + 1 .. tip + 300, arrive before the honest headers. The node
/// asks for bodies after every event, as the node loop does. It must still ask for the
/// honest bodies and reach the honest tip.
#[test]
fn junk_header_flood_does_not_starve_honest_bodies() {
    deterministic::Runner::default().start(|ctx| async move {
        let mut now = T0;
        let (src, mut dst, honest) = pair(&ctx, 5, 40, &mut now).await;
        let tip = dst.tip_height();
        assert_eq!(tip, 5);
        let mut junk = HashSet::new();
        let mut asked_junk = 0;
        for i in 0..600u64 {
            let height = if i < 300 { tip + 1 } else { tip + 1 + (i - 300) };
            let (h, proof) = junk_header(i, height);
            junk.insert(h.hash());
            let r = dst.insert_header(h, proof).await.unwrap();
            assert_eq!(r, HeaderAccept::Stored { need_parent: Some(h.prev_hash) }, "junk header {i}");
            asked_junk += dst.next_bodies(BODY_WINDOW, now).len();
        }
        assert_eq!(dst.headers_only_count(), 600);
        assert_eq!(asked_junk, 0, "bodies asked for headers above unknown ancestry");
        assert_eq!(dst.bodies_outstanding(), 0);

        // the honest peer's headers arrive (tip first); nothing is anchored until the lowest
        // one names a live parent
        backfill_headers(&mut dst, &honest).await;
        let want: Vec<Hash> = honest.iter().map(|b| b.hash()).collect();
        let asked = dst.next_bodies(BODY_WINDOW, now);
        assert!(asked.iter().all(|h| !junk.contains(h)), "a junk body was asked for");
        assert_eq!(asked, want, "every honest body, lowest height first");

        // the bodies arrive in the order asked and the tip advances to the honest tip
        for (h, b) in asked.iter().zip(&honest) {
            assert_eq!(*h, b.hash());
            let (a, _) = dst.insert_block(b.clone(), now).await.unwrap();
            assert_eq!(a, Accept::Added { tip: true });
        }
        assert_eq!(dst.tip().hash, src.tip().hash);
        assert_eq!(dst.tip_height(), 45);
        assert_eq!(dst.bodies_outstanding(), 0);
        // the junk never gets a slot, however long it waits
        assert!(dst.expire_bodies(now + 10 * BODY_ASK_TIMEOUT_MS).is_empty());
        assert!(dst.next_bodies(BODY_WINDOW, now + 10 * BODY_ASK_TIMEOUT_MS).is_empty());
    });
}

/// Anchoring follows the parent links: headers held above a gap are not scheduled, and when
/// the missing parent arrives its whole held subtree is, including headers that hang off an
/// orphan block.
#[test]
fn anchoring_reaches_children_when_the_missing_parent_arrives() {
    deterministic::Runner::default().start(|ctx| async move {
        let mut now = T0;
        let (_src, mut dst, b) = pair(&ctx, 2, 6, &mut now).await;
        // heights 3..8 are b[0]..b[5]. Headers 5, 6 and 8 arrive; block 7 arrives by gossip.
        for i in [5, 3, 2] {
            dst.insert_header(b[i].header, b[i].proof.clone()).await.unwrap();
        }
        let (a, _) = dst.insert_block(b[4].clone(), now).await.unwrap();
        assert_eq!(a, Accept::Orphan { missing: b[3].hash() });
        assert!(b.iter().all(|x| !dst.is_anchored(&x.hash())));
        assert!(dst.next_bodies(BODY_WINDOW, now).is_empty(), "nothing connects to a live block yet");
        // header 3 names the live tip: it is anchored, 4 is still missing
        dst.insert_header(b[0].header, b[0].proof.clone()).await.unwrap();
        assert!(dst.is_anchored(&b[0].hash()) && !dst.is_anchored(&b[2].hash()));
        assert_eq!(dst.next_bodies(BODY_WINDOW, now), vec![b[0].hash()]);
        // header 4 closes the gap: 5, 6, the orphan 7 and header 8 above it are anchored
        dst.insert_header(b[1].header, b[1].proof.clone()).await.unwrap();
        assert!(b.iter().all(|x| dst.is_anchored(&x.hash())));
        assert_eq!(dst.next_bodies(BODY_WINDOW, now), vec![b[1].hash(), b[2].hash(), b[3].hash(), b[5].hash()]);
        // bodies arrive out of order: all connect
        for i in [5, 3, 2, 1, 0] {
            dst.insert_block(b[i].clone(), now).await.unwrap();
        }
        assert_eq!(dst.tip().hash, b[5].hash());
        assert_eq!((dst.headers_only_count(), dst.orphan_count(), dst.bodies_outstanding()), (0, 0, 0));
        assert!(b.iter().all(|x| !dst.is_anchored(&x.hash())), "live blocks leave the anchored set");
    });
}

/// A block that arrives by gossip with no header first connects, and the headers that were
/// waiting on it are scheduled.
#[test]
fn headers_waiting_on_a_gossiped_block_are_scheduled_when_it_connects() {
    deterministic::Runner::default().start(|ctx| async move {
        let mut now = T0;
        let (_src, mut dst, b) = pair(&ctx, 2, 3, &mut now).await;
        dst.insert_header(b[2].header, b[2].proof.clone()).await.unwrap();
        dst.insert_header(b[1].header, b[1].proof.clone()).await.unwrap();
        assert!(dst.next_bodies(BODY_WINDOW, now).is_empty());
        let (a, _) = dst.insert_block(b[0].clone(), now).await.unwrap();
        assert_eq!(a, Accept::Added { tip: true });
        assert_eq!(dst.next_bodies(BODY_WINDOW, now), vec![b[1].hash(), b[2].hash()]);
    });
}

/// Anchored headers whose bodies nobody serves (a real branch whose blocks are withheld) hold
/// the window only until their asks expire: the honest headers behind them are asked first
/// then, and the expired ones come back on a share of the window.
#[test]
fn unserved_body_asks_expire_and_requeue_behind_the_others() {
    deterministic::Runner::default().start(|ctx| async move {
        let window = 8;
        let mut now = T0;
        let (_src, mut dst, honest) = pair(&ctx, 3, 6, &mut now).await;
        // a second real branch of 8 blocks on the same parent, built elsewhere and never served
        let mut other = C::open(ctx.child("other"), "o", genesis(), MockVerifier).await.unwrap();
        for h in 1..=3 {
            let b = dst.get_block(&dst.canonical_hash(h).unwrap()).await.unwrap().unwrap();
            other.insert_block(b, now).await.unwrap();
        }
        let mut t = now;
        let withheld = extend(&mut other, dst.tip().hash, window, 9, &mut t).await;
        backfill_headers(&mut dst, &withheld).await;
        let t0 = t;
        let first = dst.next_bodies(window, t0);
        assert_eq!(first, withheld.iter().map(|b| b.hash()).collect::<Vec<_>>());
        assert_eq!(dst.bodies_outstanding(), window);

        // the honest headers arrive: the window is full of asks nobody answers
        backfill_headers(&mut dst, &honest).await;
        assert!(dst.next_bodies(window, t0 + 1).is_empty());
        assert!(dst.expire_bodies(t0 + BODY_ASK_TIMEOUT_MS - 1).is_empty(), "not before the timeout");
        assert!(dst.next_bodies(window, t0 + BODY_ASK_TIMEOUT_MS - 1).is_empty());

        // at the timeout every slot comes back: the caller cancels those fetches
        let t1 = t0 + BODY_ASK_TIMEOUT_MS;
        let mut expired = dst.expire_bodies(t1);
        expired.sort();
        let mut all_withheld: Vec<Hash> = withheld.iter().map(|b| b.hash()).collect();
        all_withheld.sort();
        assert_eq!(expired, all_withheld);
        assert_eq!(dst.bodies_outstanding(), 0);
        // never-asked honest bodies first, then expired asks on window / BODY_RETRY_SHARE slots
        let next = dst.next_bodies(window, t1);
        let honest_hashes: Vec<Hash> = honest.iter().map(|b| b.hash()).collect();
        assert_eq!(next[..honest.len()], honest_hashes[..]);
        let retried = &next[honest.len()..];
        assert_eq!(retried.len(), window / BODY_RETRY_SHARE);
        assert!(retried.iter().all(|h| all_withheld.contains(h)));
        assert_eq!(dst.bodies_outstanding(), window);

        // the honest bodies arrive: the tip advances past the withheld branch's parent
        for b in &honest {
            let (a, _) = dst.insert_block(b.clone(), t1).await.unwrap();
            assert_eq!(a, Accept::Added { tip: true });
        }
        assert_eq!(dst.tip().hash, honest.last().unwrap().hash());
        // the retried asks expire again and rotate: the share is never exceeded, and each
        // withheld header is asked again in turn (oldest expiry first)
        assert!(dst.next_bodies(window, t1 + 1).is_empty(), "retry share in use");
        let mut seen: HashSet<Hash> = retried.iter().copied().collect();
        let mut tn = t1;
        for _ in 0..3 {
            tn += BODY_ASK_TIMEOUT_MS;
            assert_eq!(dst.expire_bodies(tn).len(), window / BODY_RETRY_SHARE);
            let again = dst.next_bodies(window, tn);
            assert_eq!(again.len(), window / BODY_RETRY_SHARE);
            assert!(again.iter().all(|h| seen.insert(*h)), "an ask came back before the others had their turn");
        }
        assert_eq!(seen.len(), window);
        // a withheld body that does arrive is still taken
        let b0 = &withheld[0];
        let (a, _) = dst.insert_block(b0.clone(), tn).await.unwrap();
        assert_eq!(a, Accept::Added { tip: false });
    });
}

/// A legitimate heavier branch that forks below the tip: its headers are anchored at the fork
/// point, the bodies above the tip are asked first, then the ones at or below it, and the node
/// reorganizes onto it.
#[test]
fn a_heavier_side_branch_is_scheduled_and_wins() {
    deterministic::Runner::default().start(|ctx| async move {
        let mut now = T0;
        let (src, mut dst, fork) = pair(&ctx, 2, 6, &mut now).await;
        // dst mines its own 3 blocks on the shared prefix (heights 3..5); src has 3'..8'
        let base = dst.tip().hash;
        let mut t = now;
        let own = extend(&mut dst, base, 3, 4, &mut t).await;
        assert_eq!(dst.tip_height(), 5);
        backfill_headers(&mut dst, &fork).await;
        let asked = dst.next_bodies(BODY_WINDOW, t);
        let h = |i: usize| fork[i].hash();
        assert_eq!(asked, vec![h(3), h(4), h(5), h(0), h(1), h(2)], "above the tip first, then the side branch below it");
        for hash in &asked {
            let b = fork.iter().find(|b| b.hash() == *hash).unwrap();
            dst.insert_block(b.clone(), t).await.unwrap();
        }
        assert_eq!(dst.tip().hash, src.tip().hash);
        assert!(!dst.is_canonical(&own[0].hash(), 3));
        assert_eq!((dst.headers_only_count(), dst.orphan_count(), dst.bodies_outstanding()), (0, 0, 0));
    });
}

/// A header that names a live parent is checked against what a block there needs (height and
/// required target), so only real work can anchor a branch.
#[test]
fn header_on_a_live_parent_needs_the_required_target_and_height() {
    deterministic::Runner::default().start(|ctx| async move {
        let mut now = T0;
        let (_src, mut dst, next) = pair(&ctx, 2, 1, &mut now).await;
        let good = next[0].header;
        let reseal = |mut h: Header| {
            let proof = MockVerifier::proof_for(&h);
            h.proof_hash = block::proof_hash(&proof);
            (h, proof)
        };
        let (h, proof) = reseal(Header { height: good.height + 3, ..good });
        let e = dst.insert_header(h, proof).await.unwrap_err();
        assert!(e.to_string().contains("height is not parent + 1"), "{e}");
        let (h, proof) = reseal(Header { target_bits: 0x2000ffff, ..good });
        assert_ne!(h.target_bits, good.target_bits);
        let e = dst.insert_header(h, proof).await.unwrap_err();
        assert!(e.to_string().contains("required"), "{e}");
        assert_eq!(dst.headers_only_count(), 0);
        let r = dst.insert_header(good, next[0].proof.clone()).await.unwrap();
        assert_eq!(r, HeaderAccept::Stored { need_parent: None });
        assert_eq!(dst.next_bodies(BODY_WINDOW, now), vec![good.hash()]);
    });
}
