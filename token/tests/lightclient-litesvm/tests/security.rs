//! Security pass 2026-10-21 (docs/SECURITY-REVIEW.md): exploit tests for the light client.
//! Each test first shows the attack's effect, then the fix path.

use lightclient::client;
use lightclient::LcError;
use lightclient_litesvm_tests::*;
use solana_sdk::signature::Signer;

fn small_params(k: u32) -> lightclient::Params {
    let mut p = params(k);
    p.fallback_m = 4;
    p.fallback_n = 8;
    p
}

/// L-1: the light client checks proof of work, not transactions. One PoW-valid block that
/// carries a FAKE PostJob (same poster + nonce, so the same job id, junk commitments) lets
/// its miner register the job id first. Before the fix the real job could never be
/// registered (the JobCommit PDA is taken) and every real job block failed verification:
/// the light client stalled for good on the first block of that job. `replace_job`
/// re-registers the job once the fake block is shown dead by the canonical header at
/// its height.
#[test]
fn l1_job_commit_squatting_by_a_fake_block_is_recoverable() {
    let p = params(4);
    let g = genesis(0x1e100000);
    let mut env = Env::new(p, &g);
    let (pk, w) = (env.payer.pubkey(), env.work.pubkey());
    use rand::{Rng, SeedableRng};
    let mut rng = rand::rngs::StdRng::seed_from_u64(11);
    let (m, n) = (64u32, 64u32);
    let k = LOCAL_K as usize;
    let a: Vec<i8> = (0..m as usize * k).map(|_| rng.random_range(-64i8..=64)).collect();
    let b: Vec<i8> = (0..k * n as usize).map(|_| rng.random_range(-64i8..=64)).collect();
    let poster = [0x31u8; 32];
    let (job_id, terms) = post_job_ids(&poster, 0, m, n, 5_000, 1_000, 500, 10);
    let cfg = pearl_job::MiningConfig::localnet_default();
    let jh = pearl_job::JobHeader { job_pubkey: job_id, terms_hash: terms, created_slot: 0, target_bits: 0 };
    let prep = pearl_job::PreparedJob::with_computed_commitments(pearl_job::Job { header: jh, config: cfg, m, n, hash_a: [0; 32], hash_b: [0; 32] }, &a, &b).unwrap();
    let real = PostJob {
        m,
        n,
        config: cfg.to_bytes(),
        hash_a: prep.job.hash_a,
        hash_b: prep.job.hash_b,
        worker_pay: 5_000,
        protocol_fee: 1_000,
        deadline: 500,
        max_blocks: 10,
        evaluators: vec![[0x41; 32]],
    };
    let fake = PostJob {
        m,
        n,
        config: cfg.to_bytes(),
        hash_a: [0xaa; 32],
        hash_b: [0xbb; 32],
        worker_pay: 5_000,
        protocol_fee: 1_000,
        deadline: 500,
        max_blocks: 10,
        evaluators: vec![[0x41; 32]],
    };
    let real_tx = tx_bytes(&poster, 0, 2, &real.body());
    let fake_tx = tx_bytes(&poster, 0, 2, &fake.body());
    let (froot, fpaths) = tx_root(&[fake_tx.clone()]);
    let (hroot, hpaths) = tx_root(&[real_tx.clone()]);

    // The attacker mines one block at height 1 (valid PoW, invalid tx) and squats the id.
    let f1 = child(&p, &g, &Work::Fallback, ChildOpts { tx_root: froot, miner: 0x66, ..Default::default() });
    env.relay(&f1).unwrap();
    assert_eq!(env.config().tip_hash, f1.hash);
    env.send(&[client::register_job(pk, &f1.hash, job_id, fake_tx.clone(), fpaths[0].clone())], &[]).unwrap();

    // The honest chain (heavier) carries the real PostJob at height 1.
    let h1 = child(&p, &g, &Work::Fallback, ChildOpts { tx_root: hroot, dt: 11_000, ..Default::default() });
    let h2 = child(&p, &h1, &Work::Fallback, ChildOpts::default());
    env.relay(&h1).unwrap();
    env.relay(&h2).unwrap();
    env.send(&[client::reorg(&h2.hash, &[h1.hash, g.hash])], &[]).unwrap();
    assert_eq!(env.config().tip_hash, h2.hash);

    // The attack's effect: the real job cannot be registered, its job block cannot verify.
    assert!(env.send(&[client::register_job(pk, &h1.hash, job_id, real_tx.clone(), hpaths[0].clone())], &[]).is_err());
    let h3 = child(&p, &h2, &Work::Job(&prep, job_id), ChildOpts::default());
    env.submit(&h3).unwrap();
    env.upload(&h3).unwrap();
    let plan = env.plan(&h3);
    assert!(env.steps(&h3, &plan).is_err(), "job block verifies against the squatted commitments");
    assert_eq!(env.status(&h3.hash), lightclient::STATUS_PENDING);

    // replace_job refuses a witness that does not show the old block dead.
    let e = env.send(&[client::replace_job(pk, &h1.hash, &f1.hash, job_id, real_tx.clone(), hpaths[0].clone())], &[]).unwrap_err();
    assert!(custom(lc_code(LcError::NotCanonical))(&e));
    let e = env.send(&[client::replace_job(pk, &h1.hash, &h2.hash, job_id, real_tx.clone(), hpaths[0].clone())], &[]).unwrap_err();
    assert!(custom(lc_code(LcError::NotCanonical))(&e), "witness at the wrong height");
    // ...and a dead block as the new source.
    let e = env.send(&[client::replace_job(pk, &f1.hash, &h1.hash, job_id, fake_tx.clone(), fpaths[0].clone())], &[]).unwrap_err();
    assert!(custom(lc_code(LcError::NotCanonical))(&e));

    // The fix: the canonical header at height 1 (h1) shows f1 dead; re-register from h1.
    env.send(&[client::replace_job(pk, &h1.hash, &h1.hash, job_id, real_tx.clone(), hpaths[0].clone())], &[]).unwrap();
    let ja = env.svm.get_account(&client::job_addr(&job_id)).unwrap();
    let jc = <lightclient::JobCommit as anchor_lang::AccountDeserialize>::try_deserialize(&mut ja.data.as_slice()).unwrap();
    assert_eq!((jc.hash_a, jc.hash_b, jc.posted_block), (prep.job.hash_a, prep.job.hash_b, h1.hash));
    // now the canonical commit cannot be replaced back by the dead block
    let e = env.send(&[client::replace_job(pk, &f1.hash, &f1.hash, job_id, fake_tx, fpaths[0].clone())], &[]).unwrap_err();
    assert!(custom(lc_code(LcError::NotCanonical))(&e));
    // the real job block verifies
    env.upload(&h3).unwrap();
    let plan = env.plan(&h3);
    env.steps(&h3, &plan).unwrap();
    assert_eq!(env.status(&h3.hash), lightclient::STATUS_VERIFIED);
    assert_eq!(env.config().tip_hash, h3.hash);
    let _ = w;
}

/// L-2: headers verified out of order (WorkDone) and then connected in one burst can move
/// the tip more than CANON_LEN (256) past the last header marked final. Before the fix
/// the skipped headers could never be marked final (they left the canonical ring) nor
/// accumulated, so the final-header accumulator stalled for good, and their relayer could
/// even close them. Now `mark_final` takes the final child as a link, and a Verified
/// non-final header outside the ring stays open until the accumulator passed it.
#[test]
fn l2_tip_jump_past_the_canonical_ring_does_not_stall_finality() {
    let p = small_params(4);
    let g = genesis(0x1e100000);
    let total = lightclient::CANON_LEN + 24;
    let mut blks: Vec<Blk> = Vec::with_capacity(total);
    for i in 0..total {
        let parent = blks.last().unwrap_or(&g).clone();
        blks.push(child(&p, &parent, &Work::Fallback, ChildOpts { miner: (i % 200) as u8 + 1, ..Default::default() }));
    }
    let mut env = Env::new(p, &g);
    let pk = env.payer.pubkey();
    // one burst, no mark_final in between (the attacker's relay, or a catch-up)
    for b in &blks {
        env.relay(b).unwrap();
    }
    let c = env.config();
    assert_eq!(c.tip_height, total as u64);
    let floor = c.final_floor;
    let ring_low = c.tip_height - (lightclient::CANON_LEN as u64 - 1);
    let old = &blks[4]; // height 5, below ring_low, canonical, never marked
    assert!(old.h.height < ring_low);
    // the effect: plain mark_final and accumulate refuse it
    assert!(custom(lc_code(LcError::NotCanonical))(&env.mark_final(&old.hash).unwrap_err()));
    let first: Vec<[u8; 32]> = blks[..1].iter().map(|b| b.hash).collect();
    assert!(custom(lc_code(LcError::NotFinal))(&env.send(&[client::accumulate(&first)], &[]).unwrap_err()));
    // and its relayer may not delete it any more (before the fix: CannotClose was not raised)
    assert!(custom(lc_code(LcError::CannotClose))(&env.send(&[client::close_header(&old.hash, pk)], &[]).unwrap_err()));
    // the fix: mark the lowest ring header final, then walk down by final-child links
    let low = &blks[(ring_low - 1) as usize];
    assert_eq!(low.h.height, ring_low);
    env.mark_final(&low.hash).unwrap();
    // a link that is not final, or not the child, is refused
    let e = env.send(&[client::mark_final_linked(&blks[0].hash, &blks[2].hash)], &[]).unwrap_err();
    assert!(custom(lc_code(LcError::NotCanonical))(&e));
    for i in (0..(ring_low - 1) as usize).rev().collect::<Vec<_>>().chunks(8) {
        let ixs: Vec<_> = i.iter().map(|&j| client::mark_final_linked(&blks[j].hash, &blks[j + 1].hash)).collect();
        env.send(&ixs, &[]).unwrap();
    }
    assert!(env.is_final(&old.hash) && env.is_final(&blks[0].hash));
    // the accumulator catches up to the floor
    let upto = floor as usize; // heights 1..=floor are blks[0..floor]
    for ch in blks[..upto].chunks(20) {
        let hs: Vec<[u8; 32]> = ch.iter().map(|b| b.hash).collect();
        env.send(&[client::accumulate(&hs)], &[]).unwrap();
    }
    let (start, count, _) = env.acc();
    assert_eq!(start + count, floor + 1);
}

/// Devnet stall 2026-10-23 at height 7497: two blocks at 7497 with equal work. The light client
/// kept the first (canonical), the node chose the other, and the node's next blocks mined jobs
/// posted in that other 7497. `begin_verify_job` wanted the posting block final or canonical, and
/// `reorg` wants strictly more work, so no header of the node's branch could ever verify: the
/// light client stopped for good. Now a Verified posting block on the job block's own branch is
/// accepted (its parent, or through Verified ancestors), never one on another branch.
#[test]
fn equal_work_fork_at_the_tip_does_not_stall_job_blocks() {
    use rand::{Rng, SeedableRng};
    let p = params(4);
    let g = genesis(0x1e100000);
    let mut env = Env::new(p, &g);
    let (pk, w) = (env.payer.pubkey(), env.work.pubkey());
    let mut rng = rand::rngs::StdRng::seed_from_u64(7497);
    let (m, n) = (64u32, 64u32);
    let k = LOCAL_K as usize;
    let a: Vec<i8> = (0..m as usize * k).map(|_| rng.random_range(-64i8..=64)).collect();
    let b: Vec<i8> = (0..k * n as usize).map(|_| rng.random_range(-64i8..=64)).collect();
    let poster = [0x52u8; 32];
    let (job_id, terms) = post_job_ids(&poster, 0, m, n, 5_000, 1_000, 500, 10);
    let cfg = pearl_job::MiningConfig::localnet_default();
    let jh = pearl_job::JobHeader { job_pubkey: job_id, terms_hash: terms, created_slot: 0, target_bits: 0 };
    let prep = pearl_job::PreparedJob::with_computed_commitments(pearl_job::Job { header: jh, config: cfg, m, n, hash_a: [0; 32], hash_b: [0; 32] }, &a, &b).unwrap();
    let post = PostJob { m, n, config: cfg.to_bytes(), hash_a: prep.job.hash_a, hash_b: prep.job.hash_b, worker_pay: 5_000, protocol_fee: 1_000, deadline: 500, max_blocks: 10, evaluators: vec![[0x41; 32]] };
    let ptx = tx_bytes(&poster, 0, 2, &post.body());
    let (root, paths) = tx_root(&[ptx.clone()]);

    let b1 = child(&p, &g, &Work::Fallback, ChildOpts::default());
    env.relay(&b1).unwrap();
    // two children of b1 with the same work: x2 reaches the light client first and stays the tip
    let x2 = child(&p, &b1, &Work::Fallback, ChildOpts { miner: 0x11, ..Default::default() });
    let y2 = child(&p, &b1, &Work::Fallback, ChildOpts { tx_root: root, miner: 0x22, ..Default::default() });
    env.relay(&x2).unwrap();
    env.relay(&y2).unwrap();
    assert_eq!(env.status(&y2.hash), lightclient::STATUS_VERIFIED);
    assert_eq!(env.config().tip_hash, x2.hash, "equal work: the first stays the tip");
    assert!(env.send(&[client::reorg(&y2.hash, &[b1.hash])], &[]).is_err(), "reorg needs strictly more work");

    // y3: a fallback block on y2 (Verified, off the tip); y4 mines the job posted in y2
    let y3 = child(&p, &y2, &Work::Fallback, ChildOpts::default());
    env.relay(&y3).unwrap();
    assert_eq!(env.config().tip_hash, x2.hash);
    let y4 = child(&p, &y3, &Work::Job(&prep, job_id), ChildOpts::default());
    env.submit(&y4).unwrap();
    let plen = y4.proof.len() as u32;
    // without the ancestors the posting block is neither canonical nor the parent: refused
    let e = env.send(&[client::begin_verify_job(pk, w, &y4.hash, &y2.hash, plen, ptx.clone(), paths[0].clone())], &[]).unwrap_err();
    assert!(custom(lc_code(LcError::NotCanonical))(&e), "{e:?}");
    // a wrong link is refused, not skipped
    let e = env.send(&[client::begin_verify_job_via(pk, w, &y4.hash, &y2.hash, &[x2.hash], plen, ptx.clone(), paths[0].clone())], &[]).unwrap_err();
    assert!(custom(lc_code(LcError::BadPath))(&e), "{e:?}");
    // a job block on the other branch cannot bind a job posted in y2
    let x3 = child(&p, &x2, &Work::Job(&prep, job_id), ChildOpts::default());
    env.submit(&x3).unwrap();
    let e = env.send(&[client::begin_verify_job_via(pk, w, &x3.hash, &y2.hash, &[x2.hash], x3.proof.len() as u32, ptx.clone(), paths[0].clone())], &[]).unwrap_err();
    assert!(custom(lc_code(LcError::BadPath))(&e), "{e:?}");
    let e = env.send(&[client::begin_verify_job(pk, w, &x3.hash, &y2.hash, x3.proof.len() as u32, ptx.clone(), paths[0].clone())], &[]).unwrap_err();
    assert!(custom(lc_code(LcError::NotCanonical))(&e), "{e:?}");

    // the fix: y2 through its Verified descendant y3; y4 verifies, its branch is heavier, reorg
    env.send(&[client::begin_verify_job_via(pk, w, &y4.hash, &y2.hash, &[y3.hash], plen, ptx.clone(), paths[0].clone())], &[]).unwrap();
    let mut off = 0;
    while off < y4.proof.len() {
        let c = client::WRITE_CHUNK.min(y4.proof.len() - off);
        env.send(&[client::write_proof(pk, w, off as u32, y4.proof[off..off + c].to_vec())], &[]).unwrap();
        off += c;
    }
    let plan = env.plan(&y4);
    env.steps(&y4, &plan).unwrap();
    assert_eq!(env.status(&y4.hash), lightclient::STATUS_VERIFIED);
    env.send(&[client::reorg(&y4.hash, &[y3.hash, y2.hash, b1.hash])], &[]).unwrap();
    assert_eq!(env.config().tip_hash, y4.hash);
}
