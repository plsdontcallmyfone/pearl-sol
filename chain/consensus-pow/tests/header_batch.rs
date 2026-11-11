//! Header batches (2026-11-11): the walk of single headers in bulk.
//!
//! A batch is the header a node asked for and its ancestors, newest first. It must not be a
//! new way to flood a node: it is bounded in count and bytes, bound to the hash asked for,
//! refused whole when it is not one chain, and every header in it passes the checks a single
//! header passes (proof of work before it is held, anchoring before its body is asked for).
//! Helpers copied from sync_flood.rs.

use agent_types::block::encode_header_with_proof;
use agent_types::tx::pubkey_from_seed;
use agent_types::*;
use commonware_runtime::{deterministic, Runner as _, Supervisor as _};
use consensus_pow::{
    check_header_links, decode_header_batch, encode_header_batch, Chain, ChainError, Genesis, HeaderAccept, JobChoice, MockVerifier, BODY_WINDOW, MAX_HEADER_BATCH,
    MAX_HEADER_BATCH_BYTES,
};
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

/// A chain of `n` job headers that link to each other and hang off an unknown random parent:
/// free to make, their work cannot be checked. Newest first, as a batch carries them.
fn junk_chain(seed: u64, n: usize, top_height: u64) -> Vec<(Header, Vec<u8>)> {
    let mut prev = [0xee; 32];
    prev[..8].copy_from_slice(&seed.to_le_bytes());
    let mut out = Vec::new();
    for i in 0..n as u64 {
        let mut proof = vec![0u8; 16];
        proof[..8].copy_from_slice(&(seed ^ i).to_le_bytes());
        let height = top_height + 1 - n as u64 + i;
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
        prev = header.hash();
        out.push((header, proof));
    }
    out.reverse();
    out
}

fn entries(batch: &[(Header, Vec<u8>)]) -> Vec<Vec<u8>> {
    batch.iter().map(|(h, p)| encode_header_with_proof(h, p)).collect()
}

fn is_invalid<T: std::fmt::Debug>(r: Result<T, ChainError>, what: &str) {
    match r {
        Err(ChainError::Invalid(_)) => {}
        other => panic!("{what}: expected an invalid batch, got {other:?}"),
    }
}

/// The honest path: 600 headers arrive in two batches instead of 600 single deliveries, and
/// the node ends up exactly where the walk of single headers leaves it (same headers held,
/// same bodies asked for, lowest first).
#[test]
fn batches_leave_the_node_where_single_headers_do() {
    deterministic::Runner::default().start(|ctx| async move {
        let mut now = T0;
        let (src, mut dst, ahead) = pair(&ctx, 5, 600, &mut now).await;
        let mut single = C::open(ctx.child("single"), "one", genesis(), MockVerifier).await.unwrap();
        for h in 1..=5 {
            let b = src.get_block(&src.canonical_hash(h).unwrap()).await.unwrap().unwrap();
            single.insert_block(b, now).await.unwrap();
        }
        // the walk of single headers: one delivery per header
        let mut singles = 0;
        let mut next = Some(ahead.last().unwrap().hash());
        while let Some(h) = next {
            let (hd, proof) = C::decode_header_payload(&src.header_with_proof(&h).await.unwrap().unwrap()).unwrap();
            singles += 1;
            next = match single.insert_header(hd, proof).await.unwrap() {
                HeaderAccept::Stored { need_parent } => need_parent,
                HeaderAccept::Known => None,
            };
        }
        assert_eq!(singles, 600);

        // the same walk in batches: the tip header singly (as a tip announcement starts it),
        // then one batch per step
        let tip = ahead.last().unwrap();
        let r = dst.insert_header(tip.header, tip.proof.clone()).await.unwrap();
        let mut need = match r {
            HeaderAccept::Stored { need_parent } => need_parent.map(|p| (p, tip.header.height - 1)),
            HeaderAccept::Known => None,
        };
        let mut batches = 0;
        let mut stored = 1;
        while let Some((anchor, height)) = need {
            let bytes = src.header_batch(&anchor, MAX_HEADER_BATCH, MAX_HEADER_BATCH_BYTES).await.unwrap().expect("the source holds it");
            assert!(bytes.len() <= MAX_HEADER_BATCH_BYTES);
            let batch = decode_header_batch(&bytes).unwrap();
            assert!(batch.len() <= MAX_HEADER_BATCH);
            assert_eq!(batch[0].0.height, height, "the height the node expects of the header it asks for");
            assert_eq!(dst.next_bodies(BODY_WINDOW, now), Vec::<Hash>::new(), "nothing is anchored before the walk connects");
            let acc = dst.insert_header_batch(&anchor, batch).await.unwrap();
            batches += 1;
            stored += acc.stored;
            need = acc.need_parent;
        }
        assert_eq!(batches, 2, "599 headers below the tip: 512 + 87");
        assert_eq!(stored, 600);
        assert_eq!(dst.headers_only_count(), single.headers_only_count());
        assert_eq!(dst.headers_only_count(), 600);
        for b in &ahead {
            assert!(dst.is_anchored(&b.hash()), "height {}", b.header.height);
        }
        let asked = dst.next_bodies(BODY_WINDOW, now);
        assert_eq!(asked, single.next_bodies(BODY_WINDOW, now));
        assert_eq!(asked, ahead[..BODY_WINDOW].iter().map(|b| b.hash()).collect::<Vec<_>>(), "lowest height first");
        assert_eq!(dst.anchored_header_top(), Some(605));
        // the bodies connect
        for b in &ahead {
            dst.insert_block(b.clone(), now).await.unwrap();
        }
        assert_eq!(dst.tip().hash, src.tip().hash);
        assert_eq!(dst.anchored_header_top(), None);
    });
}

/// A served batch is bounded: at most the count and the bytes asked, always at least the
/// header asked for, never genesis, nothing for a hash the node does not have. A batch that
/// reaches the receiver's own chain stops there.
#[test]
fn served_batches_are_bounded_and_stop_at_the_known_chain() {
    deterministic::Runner::default().start(|ctx| async move {
        let mut now = T0;
        let (src, mut dst, ahead) = pair(&ctx, 5, 40, &mut now).await;
        let tip = ahead.last().unwrap().hash();
        let one = encode_header_with_proof(&ahead[39].header, &ahead[39].proof).len() + 4;
        // count bound
        let b = decode_header_batch(&src.header_batch(&tip, 7, MAX_HEADER_BATCH_BYTES).await.unwrap().unwrap()).unwrap();
        assert_eq!(b.len(), 7);
        // byte bound: three entries fit in 6 + 3 * one, the fourth does not
        let bytes = src.header_batch(&tip, MAX_HEADER_BATCH, 6 + 3 * one + 10).await.unwrap().unwrap();
        assert!(bytes.len() <= 6 + 3 * one + 10);
        assert_eq!(decode_header_batch(&bytes).unwrap().len(), 3);
        // a byte bound below one entry still serves the header asked for
        assert_eq!(decode_header_batch(&src.header_batch(&tip, MAX_HEADER_BATCH, 1).await.unwrap().unwrap()).unwrap().len(), 1);
        // an unknown hash, and genesis alone: nothing
        assert_eq!(src.header_batch(&[9; 32], MAX_HEADER_BATCH, MAX_HEADER_BATCH_BYTES).await.unwrap(), None);
        assert_eq!(src.header_batch(&src.genesis_hash(), MAX_HEADER_BATCH, MAX_HEADER_BATCH_BYTES).await.unwrap(), None);
        // the whole chain down to height 1 (never genesis)
        let all = decode_header_batch(&src.header_batch(&tip, MAX_HEADER_BATCH, MAX_HEADER_BATCH_BYTES).await.unwrap().unwrap()).unwrap();
        assert_eq!(all.len(), 45);
        assert_eq!(all.last().unwrap().0.height, 1);
        check_header_links(&tip, &all).unwrap();
        // the receiver has heights 1..=5: it holds the 40 new headers and stops at its chain
        let acc = dst.insert_header_batch(&tip, all).await.unwrap();
        assert_eq!((acc.stored, acc.known, acc.top_height, acc.low_height, acc.need_parent), (40, 0, 45, 6, None));
        assert_eq!(dst.headers_only_count(), 40);
        assert_eq!(dst.next_bodies(BODY_WINDOW, now).len(), 40);
        // the same batch again changes nothing
        let again = decode_header_batch(&src.header_batch(&tip, MAX_HEADER_BATCH, MAX_HEADER_BATCH_BYTES).await.unwrap().unwrap()).unwrap();
        let acc = dst.insert_header_batch(&tip, again).await.unwrap();
        assert_eq!((acc.stored, acc.known, acc.need_parent), (0, 40, None));
        assert_eq!(dst.headers_only_count(), 40);
    });
}

/// Oversized batches are refused before any entry is read or held: too many entries, too many
/// bytes, a count that lies about the bytes that follow.
#[test]
fn oversized_batches_are_refused_whole() {
    deterministic::Runner::default().start(|ctx| async move {
        let mut dst = C::open(ctx.child("dst"), "d", genesis(), MockVerifier).await.unwrap();
        // 513 linked headers: one over the count
        let junk = junk_chain(1, MAX_HEADER_BATCH + 1, 2_000);
        let anchor = junk[0].0.hash();
        check_header_links(&anchor, &junk[..MAX_HEADER_BATCH]).unwrap();
        is_invalid(decode_header_batch(&encode_header_batch(&entries(&junk))), "513 entries on the wire");
        is_invalid(dst.insert_header_batch(&anchor, junk.clone()).await, "513 entries handed to the chain");
        // 100 linked headers with 16 KB proofs: over the byte bound
        let mut fat = junk_chain(2, 100, 2_000);
        for (h, p) in fat.iter_mut() {
            p.resize(16 << 10, 0xab);
            h.proof_hash = block::proof_hash(p);
        }
        let bytes = encode_header_batch(&entries(&fat));
        assert!(bytes.len() > MAX_HEADER_BATCH_BYTES);
        is_invalid(decode_header_batch(&bytes), "1.6 MiB batch");
        // one header alone may be larger than the bound (it is when served singly too)
        let mut big = junk_chain(3, 1, 2_000);
        big[0].1.resize(MAX_HEADER_BATCH_BYTES + 1000, 0xcd);
        assert_eq!(decode_header_batch(&encode_header_batch(&entries(&big))).unwrap().len(), 1);
        // a count of 512 in front of three entries, a count of 0, trailing bytes, no prefix
        let three = encode_header_batch(&entries(&junk[..3]));
        let mut lie = three.clone();
        lie[4..6].copy_from_slice(&512u16.to_le_bytes());
        is_invalid(decode_header_batch(&lie), "count above the entries present");
        let mut zero = three.clone();
        zero[4..6].copy_from_slice(&0u16.to_le_bytes());
        is_invalid(decode_header_batch(&zero), "count 0");
        let mut tail = three.clone();
        tail.push(0);
        is_invalid(decode_header_batch(&tail), "trailing bytes");
        is_invalid(decode_header_batch(&three[..5]), "five bytes");
        is_invalid(decode_header_batch(&encode_header_with_proof(&junk[0].0, &junk[0].1)), "a single header payload");
        is_invalid(dst.insert_header_batch(&anchor, Vec::new()).await, "empty batch");
        assert_eq!(dst.headers_only_count(), 0, "nothing was held");
    });
}

/// A batch with a broken link in the middle is refused whole: not even the well-linked
/// headers above the break are held, since nothing ties the rest to what was asked for.
#[test]
fn a_batch_with_a_broken_link_is_refused_whole() {
    deterministic::Runner::default().start(|ctx| async move {
        let mut now = T0;
        let (src, mut dst, ahead) = pair(&ctx, 5, 60, &mut now).await;
        let tip = ahead.last().unwrap().hash();
        let good = decode_header_batch(&src.header_batch(&tip, MAX_HEADER_BATCH, MAX_HEADER_BATCH_BYTES).await.unwrap().unwrap()).unwrap();
        // one entry missing in the middle
        let mut gap = good.clone();
        gap.remove(30);
        is_invalid(dst.insert_header_batch(&tip, gap).await, "a missing entry");
        // two entries swapped
        let mut swapped = good.clone();
        swapped.swap(20, 21);
        is_invalid(dst.insert_header_batch(&tip, swapped).await, "swapped entries");
        // an entry replaced by a valid header of another chain at the same height
        let mut other = C::open(ctx.child("other"), "o", genesis(), MockVerifier).await.unwrap();
        let g = other.genesis_hash();
        let fork = extend(&mut other, g, 65, 4, &mut now).await;
        let mut spliced = good.clone();
        let at = spliced[30].0.height as usize - 1;
        spliced[30] = (fork[at].header, fork[at].proof.clone());
        is_invalid(dst.insert_header_batch(&tip, spliced).await, "a header of another chain spliced in");
        // linked hashes with a height that does not step down by one
        let mut junk = junk_chain(5, 10, 100);
        junk.reverse();
        let mut prev = [0x11; 32];
        for (i, (h, _)) in junk.iter_mut().enumerate() {
            h.prev_hash = prev;
            if i == 4 {
                h.height += 7;
            }
            prev = h.hash();
        }
        junk.reverse();
        let a = junk[0].0.hash();
        is_invalid(dst.insert_header_batch(&a, junk).await, "heights that skip");
        assert_eq!(dst.headers_only_count(), 0, "nothing was held from any of them");
        assert_eq!(dst.next_bodies(BODY_WINDOW, now), Vec::<Hash>::new());
        // the untouched batch is taken
        assert_eq!(dst.insert_header_batch(&tip, good).await.unwrap().stored, 60);
    });
}

/// A batch of valid, well-linked headers that is not the chain the node asked for is refused
/// whole: real headers of the honest chain delivered for another hash, and a chain delivered
/// one header off.
#[test]
fn a_batch_that_does_not_start_at_the_header_asked_for_is_refused() {
    deterministic::Runner::default().start(|ctx| async move {
        let mut now = T0;
        let (src, mut dst, ahead) = pair(&ctx, 5, 60, &mut now).await;
        let tip = ahead.last().unwrap().hash();
        let good = decode_header_batch(&src.header_batch(&tip, MAX_HEADER_BATCH, MAX_HEADER_BATCH_BYTES).await.unwrap().unwrap()).unwrap();
        // asked for a hash nobody has; answered with the honest chain
        is_invalid(dst.insert_header_batch(&[0x5a; 32], good.clone()).await, "valid headers for another ask");
        // asked for the tip; answered with the chain from its parent down
        is_invalid(dst.insert_header_batch(&tip, good[1..].to_vec()).await, "the chain one header off");
        // asked for the parent; answered with the chain from the tip down
        is_invalid(dst.insert_header_batch(&ahead[58].hash(), good.clone()).await, "the chain one header early");
        assert_eq!(dst.headers_only_count(), 0);
        assert!(!dst.is_known(&tip));
    });
}

/// Proof of work is checked before a header is held, in a batch as for a single header: a
/// header with a bad proof fails the batch, and it and everything below it are not held. A
/// header that connects to the node's chain with the wrong target fails the same way.
#[test]
fn a_bad_proof_inside_a_batch_fails_it_before_the_header_is_held() {
    deterministic::Runner::default().start(|ctx| async move {
        let mut now = T0;
        let (src, mut dst, ahead) = pair(&ctx, 5, 60, &mut now).await;
        let tip = ahead.last().unwrap().hash();
        let good = decode_header_batch(&src.header_batch(&tip, MAX_HEADER_BATCH, MAX_HEADER_BATCH_BYTES).await.unwrap().unwrap()).unwrap();
        // entry 25 carries a proof that does not bind its header; proof_hash is part of the
        // header hash, so the forger rebuilds the chain above it to keep the links
        let mut forged = good.clone();
        forged.truncate(60);
        forged.reverse(); // lowest first
        let mut prev = forged[0].0.prev_hash;
        for (i, (h, p)) in forged.iter_mut().enumerate() {
            h.prev_hash = prev;
            if i == 34 {
                *p = b"mock-but-not-for-this-header-000000".to_vec();
            } else {
                *p = MockVerifier::proof_for(h);
            }
            h.proof_hash = block::proof_hash(p);
            prev = h.hash();
        }
        forged.reverse();
        let anchor = forged[0].0.hash();
        check_header_links(&anchor, &forged).unwrap();
        let bad = forged[25].0.hash();
        is_invalid(dst.insert_header_batch(&anchor, forged.clone()).await, "a bad proof in the middle");
        assert_eq!(dst.headers_only_count(), 25, "the 25 valid headers above it, as a single walk would hold");
        assert!(!dst.is_known(&bad), "the header with the bad proof was held");
        assert!(!dst.is_known(&forged[26].0.hash()));
        assert_eq!(dst.next_bodies(BODY_WINDOW, now), Vec::<Hash>::new(), "none of it is anchored: no body is asked for");
        // delivered again it fails again (the header is remembered as invalid)
        is_invalid(dst.insert_header_batch(&anchor, forged).await, "the same batch again");
        assert_eq!(dst.headers_only_count(), 25);
    });
}

/// A peer may answer its own made-up tip with a well-linked made-up chain. The batch is taken
/// (as each header would be singly) but none of it is anchored, so no body is asked for, and
/// the honest chain still syncs through it (C-U1).
#[test]
fn a_linked_junk_batch_is_held_unanchored_and_starves_nothing() {
    deterministic::Runner::default().start(|ctx| async move {
        let mut now = T0;
        let (src, mut dst, ahead) = pair(&ctx, 5, 40, &mut now).await;
        let mut held = 0;
        for seed in 0..4u64 {
            let junk = junk_chain(100 + seed, MAX_HEADER_BATCH, 6 + MAX_HEADER_BATCH as u64);
            let anchor = junk[0].0.hash();
            let lowest = junk.last().unwrap().0;
            let acc = dst.insert_header_batch(&anchor, junk).await.unwrap();
            assert_eq!(acc.stored, MAX_HEADER_BATCH);
            assert_eq!(acc.need_parent, Some((lowest.prev_hash, lowest.height - 1)));
            held += acc.stored;
            assert_eq!(dst.next_bodies(BODY_WINDOW, now), Vec::<Hash>::new(), "bodies asked for headers above unknown ancestry");
        }
        assert_eq!(dst.headers_only_count(), held);
        assert_eq!(dst.anchored_header_top(), None);
        let tip = ahead.last().unwrap().hash();
        let good = decode_header_batch(&src.header_batch(&tip, MAX_HEADER_BATCH, MAX_HEADER_BATCH_BYTES).await.unwrap().unwrap()).unwrap();
        assert_eq!(dst.insert_header_batch(&tip, good).await.unwrap().stored, 40);
        let asked = dst.next_bodies(BODY_WINDOW, now);
        assert_eq!(asked, ahead.iter().map(|b| b.hash()).collect::<Vec<_>>(), "every honest body and no other");
        for b in &ahead {
            dst.insert_block(b.clone(), now).await.unwrap();
        }
        assert_eq!(dst.tip().hash, src.tip().hash);
    });
}
