//! LiteSVM tests for token/programs/lightclient with real Pearl block proofs mined by
//! crates/pearl-job. Build first: `cd token && anchor build -p lightclient` (and
//! `-p bridge`, `-p storefront` for the proof-mode tests).
//! Run: `cargo test --release -- --nocapture` (prints the CU table).

use lightclient::client::{self, Step};
use lightclient::LcError;
use lightclient_litesvm_tests::*;
use pearl_verify_sbf as pv;
use solana_sdk::signature::Signer;

fn chain(p: &lightclient::Params, g: &Blk, n: usize, dts: &[u64]) -> Vec<Blk> {
    let mut out: Vec<Blk> = Vec::new();
    for i in 0..n {
        let parent = out.last().unwrap_or(g).clone();
        let dt = dts[i % dts.len()];
        out.push(child(p, &parent, &Work::Fallback, ChildOpts { dt, ..Default::default() }));
    }
    out
}

/// Proof hash in 8-chunk calls; the result of the last call (the one that checks).
fn proof_hash_all(env: &mut Env, b: &Blk, len: usize) -> Result<u64, solana_sdk::transaction::TransactionError> {
    let (pk, w) = (env.payer.pubkey(), env.work.pubkey());
    let calls = len.div_ceil(1024).div_ceil(8);
    for _ in 0..calls - 1 {
        env.send(&[Step::ProofHash(8).instruction(pk, w, &b.hash, pk)], &[])?;
    }
    env.send(&[Step::ProofHash(8).instruction(pk, w, &b.hash, pk)], &[])
}

#[test]
fn cu_per_step_and_cost_per_header() {
    let p = params(4);
    let g = genesis(0x1e100000);
    let b1 = child(&p, &g, &Work::Fallback, ChildOpts::default());
    let mut env = Env::new(p, &g);
    let cu_submit = env.submit(&b1).unwrap();
    env.upload(&b1).unwrap();
    let (pk, w) = (env.payer.pubkey(), env.work.pubkey());
    let mut rows: Vec<(String, u64)> = vec![("submit_header".into(), cu_submit)];
    let mut one = |env: &mut Env, s: Step| {
        let ix = s.instruction(pk, w, &b1.hash, pk);
        let cu = env.send(&[ix], &[]).unwrap_or_else(|e| panic!("{s:?}: {e:?}"));
        eprintln!("  step {s:?}: {cu}");
        rows.push((format!("{s:?}"), cu));
        cu
    };
    let chunks = b1.proof.len().div_ceil(1024) as u32;
    let ph = one(&mut env, Step::ProofHash(8));
    let mut left = chunks - 8;
    while left > 0 {
        one(&mut env, Step::ProofHash(left.min(8)));
        left = left.saturating_sub(8);
    }
    // 64 x 64 x 2048 fallback: B 2048 XOF blocks; B^T 128 chunks; A 128 chunks
    let gen = one(&mut env, Step::GenB(256));
    for _ in 0..7 {
        one(&mut env, Step::GenB(256));
    }
    let bt = one(&mut env, Step::HashBt(8));
    for _ in 0..15 {
        one(&mut env, Step::HashBt(8));
    }
    let a = one(&mut env, Step::HashA(8));
    for _ in 0..15 {
        one(&mut env, Step::HashA(8));
    }
    let oa = one(&mut env, Step::Opening(0));
    let ob = one(&mut env, Step::Opening(1));
    // 256 noise blocks per side, 16 rank blocks
    let na = one(&mut env, Step::Noise(0, 64));
    let na1 = one(&mut env, Step::Noise(0, 32));
    for _ in 0..5 {
        one(&mut env, Step::Noise(0, 32));
    }
    let nb = one(&mut env, Step::Noise(1, 64));
    let nb1 = one(&mut env, Step::Noise(1, 32));
    for _ in 0..5 {
        one(&mut env, Step::Noise(1, 32));
    }
    let j4 = one(&mut env, Step::Jackpot(4));
    let j2 = one(&mut env, Step::Jackpot(2));
    for _ in 0..4 {
        one(&mut env, Step::Jackpot(2));
    }
    let jl = one(&mut env, Step::Jackpot(2));
    assert_eq!(env.status(&b1.hash), lightclient::STATUS_VERIFIED);
    println!("\nCU per step (fallback block, 64x64x2048 r=128, 4x8 tile, proof {} bytes, {chunks} chunks):", b1.proof.len());
    for (l, c) in &rows {
        println!("  {l:<16} {c:>9}");
    }
    println!("  per GenB block {}  per HashBt chunk {}  per HashA chunk {}", gen / 256, bt / 8, a / 8);
    println!("  per proof-hash chunk ~{}  opening A {oa} (8 leaves) B^T {ob} (16 leaves)", ph / 8);
    println!(
        "  noise A: per block {} fixed {}; B^T: per block {} fixed {}",
        (na - na1) / 32,
        na1 - 32 * ((na - na1) / 32),
        (nb - nb1) / 32,
        nb1 - 32 * ((nb - nb1) / 32)
    );
    println!("  jackpot: per rank block {} fixed {} (last call incl. hash + verify {jl})", (j4 - j2) / 2, j2 - 2 * ((j4 - j2) / 2));
    let fb_total: u64 = rows.iter().filter(|(l, _)| !l.starts_with("submit")).map(|(_, c)| c).sum();
    println!("  fallback verify total {fb_total} CU");

    // Planned relay of the next header (packed txs), and a job block.
    let b2 = child(&p, &b1, &Work::Fallback, ChildOpts::default());
    let before = env.txs_sent;
    let bal0 = env.svm.get_balance(&env.payer.pubkey()).unwrap();
    env.cu_log.clear();
    env.relay(&b2).unwrap();
    let txs = env.txs_sent - before;
    let bal1 = env.svm.get_balance(&env.payer.pubkey()).unwrap();
    println!("\nplanned fallback header: {txs} txs");
    for (l, c) in &env.cu_log {
        println!("  {c:>9}  {l}");
    }
    let rent = env.svm.minimum_balance_for_rent_exemption(lightclient::layout::LEN);
    println!("  lamports spent {} (incl. header rent {rent}, refundable; reward +{})", bal0 - bal1, p.relay_reward);
    assert_eq!(env.config().tip_height, 2);
    for (l, c) in &env.cu_log {
        assert!(*c < 1_400_000, "{l} over the limit");
    }
}

#[test]
fn valid_chain_tip_finality_and_close() {
    let p = params(4);
    let g = genesis(0x1e100000);
    let blks = chain(&p, &g, 8, &[4_000, 16_000, 10_000]);
    let mut env = Env::new(p, &g);
    for b in &blks {
        env.relay(b).unwrap();
        assert_eq!(env.status(&b.hash), lightclient::STATUS_VERIFIED);
        assert_eq!(env.config().tip_hash, b.hash);
    }
    let c = env.config();
    assert_eq!(c.tip_height, 8);
    assert_eq!(c.final_floor, 4);
    assert_eq!(pv::difficulty::u256_from_le(&c.tip_work), blks[7].cum_work);
    // WTEMA really moved the target (solve times 4 s / 16 s around a 10 s target)
    assert!(blks.iter().any(|b| b.h.target_bits != g.h.target_bits));
    // finality: K deep only
    assert!(env.mark_final(&blks[4].hash).is_err());
    assert!(custom(lc_code(LcError::NotFinal))(&env.mark_final(&blks[4].hash).unwrap_err()));
    env.mark_final(&blks[3].hash).unwrap();
    env.mark_final(&blks[1].hash).unwrap();
    env.mark_final(&blks[0].hash).unwrap();
    assert!(env.is_final(&blks[3].hash) && env.is_final(&blks[1].hash));
    assert_eq!(env.config().final_height, 4);
    // close: final and retain_final (2) below the floor (4): height 1 yes, 2 not yet,
    // height 3 is canonical but not marked final: refused
    let relayer = env.payer.pubkey();
    // R5: a final header must be accumulated before it can be closed
    assert!(custom(lc_code(LcError::NotAccumulated))(&env.send(&[client::close_header(&blks[0].hash, relayer)], &[]).unwrap_err()));
    env.send(&[client::accumulate(&[blks[0].hash])], &[]).unwrap();
    assert!(custom(lc_code(LcError::CannotClose))(&env.send(&[client::close_header(&blks[1].hash, relayer)], &[]).unwrap_err()));
    assert!(custom(lc_code(LcError::CannotClose))(&env.send(&[client::close_header(&blks[2].hash, relayer)], &[]).unwrap_err()));
    let bal = env.svm.get_balance(&relayer).unwrap();
    env.send(&[client::close_header(&blks[0].hash, relayer)], &[]).unwrap();
    assert!(env.svm.get_account(&client::header_addr(&blks[0].hash)).map_or(true, |a| a.lamports == 0 && a.data.is_empty()));
    assert!(env.svm.get_balance(&relayer).unwrap() > bal);
    // relay reward was paid per verified header from the pool
    let pool = env.svm.get_balance(&client::pool_pda()).unwrap();
    assert_eq!(pool, 1_000_000_000 + env.svm.minimum_balance_for_rent_exemption(0) - 8 * p.relay_reward);
}

#[test]
fn bad_pow_is_rejected() {
    let p = params(4);
    let g = genesis(0x1e100000);
    let mut env = Env::new(p, &g);
    let (pk, w) = (env.payer.pubkey(), env.work.pubkey());

    // 1. a losing tile: proof_hash and structure fine, jackpot misses the target
    let lose = losing_block(&p, &g);
    env.submit(&lose).unwrap();
    env.upload(&lose).unwrap();
    let plan = env.plan(&lose);
    let flat: Vec<Step> = plan.concat();
    let (last, head) = flat.split_last().unwrap();
    for s in head {
        env.send(&[s.instruction(pk, w, &lose.hash, pk)], &[]).unwrap();
    }
    let e = env.send(&[last.instruction(pk, w, &lose.hash, pk)], &[]).unwrap_err();
    assert!(custom(lc_code(LcError::VerifyFailed))(&e), "{e:?}");
    let logs = env.logs_of(&[last.instruction(pk, w, &lose.hash, pk)]);
    assert!(logs.iter().any(|l| l.contains("Target")), "{logs:?}");
    assert_eq!(env.status(&lose.hash), lightclient::STATUS_PENDING);

    // 2. proof bytes that do not hash to proof_hash
    let good = child(&p, &g, &Work::Fallback, ChildOpts { miner: 7, ..Default::default() });
    env.submit(&good).unwrap();
    let mut bad = good.clone();
    let v = pv::proof::parse(&bad.proof).unwrap();
    bad.proof[v.a.data_off + 3] ^= 1;
    env.upload(&bad).unwrap();
    let e = proof_hash_all(&mut env, &good, bad.proof.len()).unwrap_err();
    assert!(custom(lc_code(LcError::VerifyFailed))(&e));

    // 3. the header's nonce changed after mining (proof_hash recomputed): nonce check
    let mut moved = child(&p, &g, &Work::Fallback, ChildOpts { miner: 8, ..Default::default() });
    moved.h.nonce += 1;
    moved.bytes = moved.h.to_bytes();
    moved.hash = moved.h.block_hash();
    env.submit(&moved).unwrap();
    env.upload(&moved).unwrap();
    let e = proof_hash_all(&mut env, &moved, moved.proof.len()).unwrap_err();
    assert!(custom(lc_code(LcError::VerifyFailed))(&e));

    // 4. steps out of order / jackpot before openings
    env.upload(&good).unwrap();
    let e = env.send(&[Step::Jackpot(1).instruction(pk, w, &good.hash, pk)], &[]).unwrap_err();
    assert!(custom(lc_code(LcError::BadStep))(&e));
    // ... and the honest proof still verifies after a fresh begin
    env.upload(&good).unwrap();
    let plan = env.plan(&good);
    env.steps(&good, &plan).unwrap();
    assert_eq!(env.status(&good.hash), lightclient::STATUS_VERIFIED);
    // children of an unverified header are refused
    let orphan = child(&p, &lose, &Work::Fallback, ChildOpts::default());
    assert!(custom(lc_code(LcError::ParentNotVerified))(&env.submit(&orphan).unwrap_err()));
}

#[test]
fn wrong_target_and_timestamps_are_rejected() {
    let p = params(4);
    let g = genesis(0x1e100000);
    let blks = chain(&p, &g, 2, &[3_000]);
    let mut env = Env::new(p, &g);
    for b in &blks {
        env.relay(b).unwrap();
    }
    let want = required_bits(&p, &blks[1]);
    assert_ne!(want, g.h.target_bits);
    // easier than WTEMA allows (and mined at that easier target)
    let easy = child(&p, &blks[1], &Work::Fallback, ChildOpts { bits: Some(0x1f00ffff), ..Default::default() });
    assert!(custom(lc_code(LcError::WrongTarget))(&env.submit(&easy).unwrap_err()));
    // the parent's target instead of the retargeted one
    let stale = child(&p, &blks[1], &Work::Fallback, ChildOpts { bits: Some(blks[1].h.target_bits), ..Default::default() });
    assert!(custom(lc_code(LcError::WrongTarget))(&env.submit(&stale).unwrap_err()));
    // timestamp not after the median of the last 11
    let old = child(&p, &blks[1], &Work::Fallback, ChildOpts { ts: Some(blks[0].h.timestamp_ms), ..Default::default() });
    assert!(custom(lc_code(LcError::Timestamp))(&env.submit(&old).unwrap_err()));
    // too far ahead of the Solana clock
    let clock_ms = (T0 / 1000 + 1_000_000) * 1000;
    let future = child(&p, &blks[1], &Work::Fallback, ChildOpts { ts: Some(clock_ms + p.max_future_ms + 1), ..Default::default() });
    assert!(custom(lc_code(LcError::Timestamp))(&env.submit(&future).unwrap_err()));
    // a header hash that does not match its bytes
    let ok = child(&p, &blks[1], &Work::Fallback, ChildOpts::default());
    let mut ix = client::submit_header(env.payer.pubkey(), ok.bytes);
    let mut forged = ok.bytes;
    forged[200] ^= 1;
    ix.data = anchor_lang::InstructionData::data(&lightclient::instruction::SubmitHeader { hash: ok.hash, header: forged });
    assert!(env.send(&[ix], &[]).is_err());
    env.relay(&ok).unwrap();
}

#[test]
fn fork_with_more_work_wins_and_deep_reorgs_are_refused() {
    let p = params(4);
    let g = genesis(0x1e100000);
    let main = chain(&p, &g, 4, &[10_000]);
    let mut env = Env::new(p, &g);
    for b in &main {
        env.relay(b).unwrap();
    }
    assert_eq!(env.config().tip_height, 4);
    // Branch from height 2: 3 blocks (heights 3, 4, 5) with different payout keys.
    let mut side = vec![main[1].clone()];
    for i in 0..3 {
        let parent = side.last().unwrap().clone();
        side.push(child(&p, &parent, &Work::Fallback, ChildOpts { miner: 0x10 + i, ..Default::default() }));
    }
    let side = &side[1..];
    env.relay(&side[0]).unwrap();
    env.relay(&side[1]).unwrap();
    // equal work at height 4: the first seen tip stays
    assert_eq!(env.config().tip_hash, main[3].hash);
    let e = env.send(&[client::reorg(&side[1].hash, &[side[0].hash, main[1].hash])], &[]).unwrap_err();
    assert!(custom(lc_code(LcError::NotHeavier))(&e));
    env.relay(&side[2]).unwrap();
    assert_eq!(env.config().tip_hash, main[3].hash, "verified but not yet switched");
    // bad path (skips a header) is refused
    let e = env.send(&[client::reorg(&side[2].hash, &[side[0].hash, main[1].hash])], &[]).unwrap_err();
    assert!(custom(lc_code(LcError::BadPath))(&e));
    env.send(&[client::reorg(&side[2].hash, &[side[1].hash, side[0].hash, main[1].hash])], &[]).unwrap();
    let c = env.config();
    assert_eq!((c.tip_hash, c.tip_height), (side[2].hash, 5));
    assert_eq!(c.final_floor, 1);
    // the old branch's header 3 is no longer canonical: cannot be marked final later
    // extend the side branch so floor passes 3, then main[2] (height 3) is not final
    let mut tipb = side[2].clone();
    for i in 0..3 {
        tipb = child(&p, &tipb, &Work::Fallback, ChildOpts { miner: 0x20 + i, ..Default::default() });
        env.relay(&tipb).unwrap();
    }
    assert_eq!(env.config().final_floor, 4);
    assert!(custom(lc_code(LcError::NotCanonical))(&env.mark_final(&main[2].hash).unwrap_err()));
    env.mark_final(&side[0].hash).unwrap();
    // a fork below the final floor cannot even be submitted
    let deep = child(&p, &main[2], &Work::Fallback, ChildOpts { miner: 0x77, ..Default::default() });
    assert!(custom(lc_code(LcError::TooDeep))(&env.submit(&deep).unwrap_err()));
    // a fork at the floor with more work is still refused if its fork point is below the floor
    // (fork point main[1] = height 2 < floor 4): build it from side[0]'s parent chain is impossible
    // without submitting at height <= floor, which the line above shows is refused.
}

#[test]
fn job_block_needs_its_registered_job() {
    let p = params(4);
    let g = genesis(0x1e100000);
    let mut env = Env::new(p, &g);
    // A real job: random A, B; PostJob tx in block 1.
    use rand::{Rng, SeedableRng};
    let mut rng = rand::rngs::StdRng::seed_from_u64(5);
    let (m, n) = (64u32, 64u32);
    let k = LOCAL_K as usize;
    let a: Vec<i8> = (0..m as usize * k).map(|_| rng.random_range(-64i8..=64)).collect();
    let b: Vec<i8> = (0..k * n as usize).map(|_| rng.random_range(-64i8..=64)).collect();
    let poster = [0x31u8; 32];
    let (job_id, terms) = post_job_ids(&poster, 0, m, n, 5_000, 1_000, 500, 10);
    let cfg = pearl_job::MiningConfig::localnet_default();
    let jh = pearl_job::JobHeader { job_pubkey: job_id, terms_hash: terms, created_slot: 0, target_bits: 0 };
    let prep = pearl_job::PreparedJob::with_computed_commitments(pearl_job::Job { header: jh, config: cfg, m, n, hash_a: [0; 32], hash_b: [0; 32] }, &a, &b).unwrap();
    let pj = PostJob {
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
    let post = tx_bytes(&poster, 0, 2, &pj.body());
    let other = tx_bytes(&[0x32; 32], 3, 0, &[[9u8; 32].as_slice(), &7u64.to_le_bytes()].concat());
    let (root, paths) = tx_root(&[other.clone(), post.clone()]);
    let b1 = child(&p, &g, &Work::Fallback, ChildOpts { tx_root: root, ..Default::default() });
    env.relay(&b1).unwrap();
    // A job block before the job is registered: begin_verify refuses.
    let b2 = child(&p, &b1, &Work::Job(&prep, job_id), ChildOpts::default());
    env.submit(&b2).unwrap();
    let (pk, w) = (env.payer.pubkey(), env.work.pubkey());
    let e = env.send(&[client::begin_verify(pk, w, &b2.hash, Some(&job_id), b2.proof.len() as u32)], &[]).unwrap_err();
    assert!(matches!(e, solana_sdk::transaction::TransactionError::InstructionError(..)), "{e:?}");
    // Wrong tx path, wrong job id: refused.
    let e = env.send(&[client::register_job(pk, &b1.hash, job_id, post.clone(), paths[0].clone())], &[]).unwrap_err();
    assert!(custom(lc_code(LcError::VerifyFailed))(&e));
    let e = env.send(&[client::register_job(pk, &b1.hash, [1; 32], post.clone(), paths[1].clone())], &[]).unwrap_err();
    assert!(matches!(e, solana_sdk::transaction::TransactionError::InstructionError(..)));
    let cu = env.send(&[client::register_job(pk, &b1.hash, job_id, post.clone(), paths[1].clone())], &[]).unwrap();
    println!("register_job: {cu} CU");
    let ja = env.svm.get_account(&client::job_addr(&job_id)).unwrap();
    let jc = <lightclient::JobCommit as anchor_lang::AccountDeserialize>::try_deserialize(&mut ja.data.as_slice()).unwrap();
    assert_eq!(jc.job_key, prep.job.job_key());
    assert_eq!((jc.hash_a, jc.hash_b), (prep.job.hash_a, prep.job.hash_b));
    // Job block verifies now (2 or 3 txs, no fallback hashing).
    env.cu_log.clear();
    env.upload(&b2).unwrap();
    let plan = env.plan(&b2);
    println!("job block plan: {plan:?}");
    env.steps(&b2, &plan).unwrap();
    for (l, c) in &env.cu_log {
        println!("  {c:>9}  {l}");
    }
    assert_eq!(env.status(&b2.hash), lightclient::STATUS_VERIFIED);
    assert_eq!(env.config().tip_hash, b2.hash);
    // A job block whose proof is for another job's commitments fails the openings.
    let other_prep = {
        let jh = pearl_job::JobHeader { job_pubkey: [0x55; 32], terms_hash: terms, created_slot: 0, target_bits: 0 };
        pearl_job::PreparedJob::with_computed_commitments(pearl_job::Job { header: jh, config: cfg, m, n, hash_a: [0; 32], hash_b: [0; 32] }, &a, &b).unwrap()
    };
    let b3 = child(&p, &b2, &Work::Job(&other_prep, job_id), ChildOpts::default());
    env.submit(&b3).unwrap();
    env.upload(&b3).unwrap();
    let r = env.steps(&b3, &env.plan(&b3));
    assert!(r.is_err());
    assert_eq!(env.status(&b3.hash), lightclient::STATUS_PENDING);
}

/// Single-leaf and two-leaf Commonware MMR roots (BackwardFold, no inactive peaks),
/// matching the verifier that tests/crosscheck.rs checks against commonware-storage.
fn mmr_two(e0: &[u8], e1: &[u8]) -> ([u8; 32], Vec<u8>, Vec<u8>) {
    use sha2::{Digest, Sha256};
    let h = |parts: &[&[u8]]| -> [u8; 32] {
        let mut s = Sha256::new();
        for p in parts {
            s.update(p);
        }
        s.finalize().into()
    };
    let l0 = h(&[&0u64.to_be_bytes(), e0]);
    let l1 = h(&[&1u64.to_be_bytes(), e1]);
    let node = h(&[&2u64.to_be_bytes(), &l0, &l1]);
    let root = h(&[&2u64.to_be_bytes(), &node]);
    // proof for loc 0: sibling l1; for loc 1: sibling l0
    let p0 = [vec![2u8, 0, 1], l1.to_vec()].concat();
    let p1 = [vec![2u8, 0, 1], l0.to_vec()].concat();
    (root, p0, p1)
}

fn append_el(entry: &[u8]) -> Vec<u8> {
    let mut v = vec![1u8, entry.len() as u8];
    v.extend_from_slice(entry);
    v
}

fn entry(nonce: u64, to: &solana_sdk::pubkey::Pubkey, amount: u64) -> Vec<u8> {
    let mut e = nonce.to_le_bytes().to_vec();
    e.extend_from_slice(&[0x66; 32]);
    e.extend_from_slice(to.as_ref());
    e.extend_from_slice(&amount.to_le_bytes());
    e.extend_from_slice(&40u64.to_le_bytes());
    e
}

#[test]
fn bridge_release_by_proof() {
    let p = params(3);
    let g = genesis(0x1e100000);
    let mut env = Env::new(p, &g);
    let bs = env.setup_bridge(10_000_000, bridge::MODE_SIGNERS);
    let alice = solana_sdk::signature::Keypair::new();
    let bob = solana_sdk::signature::Keypair::new();
    let alice_t = env.create_ata(&alice.pubkey(), &bs.mint);
    let bob_t = env.create_ata(&bob.pubkey(), &bs.mint);
    let e0 = entry(0, &alice.pubkey(), 2_000);
    let e1 = entry(1, &bob.pubkey(), 3_000);
    let (root, p0, p1) = mmr_two(&append_el(&e0), &append_el(&e1));
    let mut blks = vec![child(&p, &g, &Work::Fallback, ChildOpts { withdraw_root: root, ..Default::default() })];
    for _ in 0..3 {
        let parent = blks.last().unwrap().clone();
        blks.push(child(&p, &parent, &Work::Fallback, ChildOpts { withdraw_root: root, ..Default::default() }));
    }
    for b in &blks {
        env.relay(b).unwrap();
    }
    let hdr = blks[0].hash;
    // proof mode disabled
    let ix = env.release_proof_ix(&bs, 0, &e0, 0, p0.clone(), &hdr, alice_t);
    assert!(custom(u32::from(bridge::BridgeError::ModeDisabled))(&env.send(&[ix], &[]).unwrap_err()));
    env.set_release_modes(bridge::MODE_PROOF).unwrap();
    // header verified but not marked final
    let ix = env.release_proof_ix(&bs, 0, &e0, 0, p0.clone(), &hdr, alice_t);
    assert!(custom(u32::from(bridge::BridgeError::BadHeader))(&env.send(&[ix], &[]).unwrap_err()));
    env.mark_final(&hdr).unwrap();
    // forged: wrong amount in the entry, wrong location, wrong proof, wrong recipient
    let mut forged = e0.clone();
    forged[72] = 0xff;
    let ix = env.release_proof_ix(&bs, 0, &forged, 0, p0.clone(), &hdr, alice_t);
    assert!(custom(u32::from(bridge::BridgeError::BadProof))(&env.send(&[ix], &[]).unwrap_err()));
    let ix = env.release_proof_ix(&bs, 0, &e0, 1, p0.clone(), &hdr, alice_t);
    assert!(custom(u32::from(bridge::BridgeError::BadProof))(&env.send(&[ix], &[]).unwrap_err()));
    let ix = env.release_proof_ix(&bs, 0, &e0, 0, p1.clone(), &hdr, alice_t);
    assert!(custom(u32::from(bridge::BridgeError::BadProof))(&env.send(&[ix], &[]).unwrap_err()));
    let ix = env.release_proof_ix(&bs, 0, &e0, 0, p0.clone(), &hdr, bob_t);
    assert!(custom(u32::from(bridge::BridgeError::WrongRecipient))(&env.send(&[ix], &[]).unwrap_err()));
    // a non-light-client account posing as a header
    let mut ix = env.release_proof_ix(&bs, 0, &e0, 0, p0.clone(), &hdr, alice_t);
    ix.accounts[6].pubkey = env.payer.pubkey();
    assert!(custom(u32::from(bridge::BridgeError::BadHeader))(&env.send(&[ix], &[]).unwrap_err()));
    // the real one pays exactly once
    let ix = env.release_proof_ix(&bs, 0, &e0, 0, p0.clone(), &hdr, alice_t);
    let cu = env.send(&[ix.clone()], &[]).unwrap();
    println!("release_proof: {cu} CU");
    assert_eq!(env.token_balance(&alice_t), 2_000);
    assert!(env.send(&[ix], &[]).is_err(), "replay");
    // signer path is now off: even a valid signature cannot pay withdrawal 1
    let msg = bridge::bridge_message(bridge::WITHDRAW_DOMAIN, 1, &bob.pubkey(), 3_000);
    let sig = bs.signer.sign_message(&msg);
    let ed = ed25519_ix(&[(bs.signer.pubkey().to_bytes(), sig.as_ref().try_into().unwrap(), msg)]);
    let rel = env.release_signed_ix(&bs, 1, bob.pubkey(), 3_000, bob_t);
    assert!(custom(u32::from(bridge::BridgeError::ModeDisabled))(&env.send(&[ed.clone(), rel.clone()], &[]).unwrap_err()));
    // both modes: the signer path pays 1, and then the proof path cannot pay it again
    env.set_release_modes(bridge::MODE_SIGNERS | bridge::MODE_PROOF).unwrap();
    env.send(&[ed, rel], &[]).unwrap();
    assert_eq!(env.token_balance(&bob_t), 3_000);
    let ix = env.release_proof_ix(&bs, 1, &e1, 1, p1, &hdr, bob_t);
    assert!(env.send(&[ix], &[]).is_err(), "already released by signers");
    assert_eq!(env.token_balance(&bob_t), 3_000);
}

#[test]
fn storefront_finalize_and_expire_by_proof() {
    let p = params(3);
    let g = genesis(0x1e100000);
    let mut env = Env::new(p, &g);
    let attester = solana_sdk::signature::Keypair::new();
    env.setup_storefront(&attester.pubkey(), storefront::MODE_SIGNERS);
    let evaluator = solana_sdk::signature::Keypair::new();
    let buyer = env.payer.pubkey();
    env.inject_order(7, &buyer, &evaluator.pubkey());
    env.inject_order(8, &buyer, &evaluator.pubkey());
    let job7 = storefront::order_job_id(7);
    let job8 = storefront::order_job_id(8);
    let deliverer = solana_sdk::pubkey::Pubkey::new_unique();
    let result_hash = [0xabu8; 32];
    let msg = storefront::result_message(&job7, &deliverer, &result_hash);
    let sig: [u8; 64] = evaluator.sign_message(&msg).as_ref().try_into().unwrap();
    let mut body = job7.to_vec();
    body.push(1);
    body.extend_from_slice(evaluator.pubkey().as_ref());
    body.extend_from_slice(&sig);
    let complete = tx_bytes(&[0x51; 32], 0, 4, &body);
    let expire = tx_bytes(&[0x52; 32], 0, 5, &job8);
    let filler = tx_bytes(&[0x53; 32], 1, 0, &[[1u8; 32].as_slice(), &5u64.to_le_bytes()].concat());
    let (root, paths) = tx_root(&[filler, complete.clone(), expire.clone()]);
    let mut blks = vec![child(&p, &g, &Work::Fallback, ChildOpts { tx_root: root, ..Default::default() })];
    for _ in 0..3 {
        let parent = blks.last().unwrap().clone();
        blks.push(child(&p, &parent, &Work::Fallback, ChildOpts::default()));
    }
    for b in &blks {
        env.relay(b).unwrap();
    }
    let hdr = blks[0].hash;
    let fin = |nonce: u64, deliverer, result_hash, tx: Vec<u8>, path: Vec<u8>| solana_sdk::instruction::Instruction {
        program_id: storefront::ID,
        accounts: anchor_lang::ToAccountMetas::to_account_metas(
            &storefront::accounts::CloseOrderProof {
                caller: buyer,
                config: sf_config(),
                order: storefront::order_pda(nonce).0,
                instructions: IX_SYSVAR,
                lc_header: client::header_addr(&hdr),
            },
            None,
        ),
        data: anchor_lang::InstructionData::data(&storefront::instruction::FinalizeOrderProof {
            deliverer,
            result_hash,
            result_uri: "blob:result".into(),
            tx,
            path,
            pair: 0,
        }),
    };
    let ed = ed25519_ix(&[(evaluator.pubkey().to_bytes(), sig, msg.clone())]);
    // proof mode off
    let e = env.send(&[ed.clone(), fin(7, deliverer, result_hash, complete.clone(), paths[1].clone())], &[]).unwrap_err();
    assert!(custom(u32::from(storefront::StorefrontError::ModeDisabled))(&e));
    env.set_finalize_modes(storefront::MODE_PROOF).unwrap();
    // not final yet
    let e = env.send(&[ed.clone(), fin(7, deliverer, result_hash, complete.clone(), paths[1].clone())], &[]).unwrap_err();
    assert!(custom(u32::from(storefront::StorefrontError::BadProof))(&e));
    env.mark_final(&hdr).unwrap();
    // wrong path; a different result (evaluator signs another hash off chain); no precompile
    let e = env.send(&[ed.clone(), fin(7, deliverer, result_hash, complete.clone(), paths[0].clone())], &[]).unwrap_err();
    assert!(custom(u32::from(storefront::StorefrontError::BadProof))(&e));
    let other = [0xcdu8; 32];
    let msg2 = storefront::result_message(&job7, &deliverer, &other);
    let sig2: [u8; 64] = evaluator.sign_message(&msg2).as_ref().try_into().unwrap();
    let ed2 = ed25519_ix(&[(evaluator.pubkey().to_bytes(), sig2, msg2)]);
    let e = env.send(&[ed2, fin(7, deliverer, other, complete.clone(), paths[1].clone())], &[]).unwrap_err();
    assert!(custom(u32::from(storefront::StorefrontError::NotEnoughSignatures))(&e));
    let e = env.send(&[fin(7, deliverer, result_hash, complete.clone(), paths[1].clone())], &[]).unwrap_err();
    assert!(custom(u32::from(storefront::StorefrontError::NotEnoughSignatures))(&e));
    // the CompleteJob for order 7 does not finalize order 8
    let e = env.send(&[ed.clone(), fin(8, deliverer, result_hash, complete.clone(), paths[1].clone())], &[]).unwrap_err();
    assert!(custom(u32::from(storefront::StorefrontError::BadProof))(&e));
    let cu = env.send(&[ed.clone(), fin(7, deliverer, result_hash, complete.clone(), paths[1].clone())], &[]).unwrap();
    println!("finalize_order_proof: {cu} CU");
    let o = env.order(7);
    assert_eq!((o.status, o.deliverer, o.result_hash), (storefront::STATUS_FINALIZED, deliverer, result_hash));
    assert!(env.send(&[ed, fin(7, deliverer, result_hash, complete, paths[1].clone())], &[]).is_err(), "closed for good");
    // expire by proof
    let exp = solana_sdk::instruction::Instruction {
        program_id: storefront::ID,
        accounts: anchor_lang::ToAccountMetas::to_account_metas(
            &storefront::accounts::CloseOrderProof {
                caller: buyer,
                config: sf_config(),
                order: storefront::order_pda(8).0,
                instructions: IX_SYSVAR,
                lc_header: client::header_addr(&hdr),
            },
            None,
        ),
        data: anchor_lang::InstructionData::data(&storefront::instruction::ExpireOrderProof { tx: expire, path: paths[2].clone() }),
    };
    env.send(&[exp], &[]).unwrap();
    assert_eq!(env.order(8).status, storefront::STATUS_EXPIRED);
}

/// Owner decision 2026-10-17: shrink fallback matrices via the chain parameters
/// fallback_m / fallback_n. 4 x 8 is the smallest shape the 4x8 tile allows.
#[test]
fn cu_small_fallback_4x8() {
    let mut p = params(4);
    p.fallback_m = 4;
    p.fallback_n = 8;
    let g = genesis(0x1e100000);
    let blks = chain(&p, &g, 3, &[10_000]);
    let mut env = Env::new(p, &g);
    env.relay(&blks[0]).unwrap();
    env.cu_log.clear();
    let before = env.txs_sent;
    env.relay(&blks[1]).unwrap();
    let total: u64 = env.cu_log.iter().map(|(_, c)| c).sum();
    println!("\n4x8 fallback header: {} txs, {} CU total", env.txs_sent - before, total);
    for (l, c) in env.cu_log.iter().filter(|(l, _)| l != "write_proof") {
        println!("  {c:>9}  {l}");
    }
    env.relay(&blks[2]).unwrap();
    assert_eq!(env.config().tip_height, 3);
}

fn sha(parts: &[&[u8]]) -> [u8; 32] {
    use sha2::Digest;
    let mut h = sha2::Sha256::new();
    for p in parts {
        h.update(p);
    }
    h.finalize().into()
}

/// R5: final headers go into the accumulator in order (final, or canonical at or below
/// the floor; linked by prev_hash); closed headers stay provable (`prove_header`, CU
/// measured) and can be restored as readable final records (`restore_header`) that the
/// raw reader accepts; proofs against recent sizes still verify; tampering fails.
#[test]
fn final_header_accumulator_outlives_close_header() {
    let p = params(3);
    let g = genesis(0x1e100000);
    let blks = chain(&p, &g, 20, &[10_000]);
    let mut env = Env::new(p, &g);
    for b in &blks {
        env.relay(b).unwrap();
    }
    let floor = env.config().final_floor;
    assert_eq!(floor, 17);
    assert_eq!(env.acc(), (0, 1, g.hash));
    // out of order / not linked / not final yet: refused
    assert!(custom(lc_code(LcError::AccOrder))(&env.send(&[client::accumulate(&[blks[1].hash])], &[]).unwrap_err()));
    // canonical at or below the floor counts as final even before mark_final
    let first: Vec<[u8; 32]> = blks[..10].iter().map(|b| b.hash).collect();
    let cu = env.send(&[client::accumulate(&first)], &[]).unwrap();
    println!("accumulate 10 headers: {cu} CU");
    let rest: Vec<[u8; 32]> = blks[10..17].iter().map(|b| b.hash).collect();
    env.send(&[client::accumulate(&rest)], &[]).unwrap();
    assert!(custom(lc_code(LcError::NotFinal))(&env.send(&[client::accumulate(&[blks[17].hash])], &[]).unwrap_err()), "height 18 is above the floor");
    let (start, count, last) = env.acc();
    assert_eq!((start, count, last), (0, 18, blks[16].hash));
    // close old final headers (height + retain_final 2 < floor 17)
    for b in &blks[..12] {
        env.mark_final(&b.hash).unwrap();
    }
    let relayer = env.payer.pubkey();
    // (blks[11] is the highest marked final: config.final_hash, never closed)
    for b in &blks[..11] {
        env.send(&[client::close_header(&b.hash, relayer)], &[]).unwrap();
        assert!(env.svm.get_account(&client::header_addr(&b.hash)).map_or(true, |a| a.data.is_empty()));
    }
    // every header from the checkpoint on is provable, at the current size
    let mut hashes = vec![g.hash];
    hashes.extend(blks[..17].iter().map(|b| b.hash));
    let mut worst = 0;
    for (i, b) in blks[..17].iter().enumerate() {
        let short = pv::acc::prove_short(sha, &hashes, i as u64 + 1).unwrap();
        let (cu_s, ret_s) = env.send_return(&[client::prove_header(b.bytes, count, short.clone())]).unwrap();
        assert_eq!(&ret_s[..32], &b.hash);
        let proof = pv::acc::prove(sha, &hashes, i as u64 + 1).unwrap();
        let (cu, ret) = env.send_return(&[client::prove_header(b.bytes, count, proof.clone())]).unwrap();
        worst = worst.max(cu).max(cu_s);
        assert_eq!(&ret[..32], &b.hash);
        assert_eq!(u64::from_le_bytes(ret[32..40].try_into().unwrap()), b.h.height);
        // the raw reader agrees
        let acc_data = env.svm.get_account(&client::acc_addr()).unwrap().data;
        assert_eq!(lightclient::verify_accumulated(&acc_data, &b.bytes, count, &proof), Some((b.hash, b.h.height)));
        assert_eq!(lightclient::verify_accumulated(&acc_data, &b.bytes, count, &short), Some((b.hash, b.h.height)));
        let mut bad = b.bytes;
        bad[250] ^= 1;
        assert!(env.send(&[client::prove_header(bad, count, proof.clone())], &[]).is_err());
        assert!(lightclient::verify_accumulated(&acc_data, &bad, count, &proof).is_none());
    }
    println!("prove_header (18 leaves): max {worst} CU");
    // a proof built at an older size (10 leaves) still verifies after appends (root ring)
    let old = pv::acc::prove(sha, &hashes[..10], 3).unwrap();
    env.send(&[client::prove_header(blks[2].bytes, 10, old.clone())], &[]).unwrap();
    assert!(custom(lc_code(LcError::AccRoot))(&env.send(&[client::prove_header(blks[2].bytes, 19, old.clone())], &[]).unwrap_err()));
    assert!(custom(lc_code(LcError::AccProof))(&env.send(&[client::prove_header(blks[2].bytes, 11, old)], &[]).unwrap_err()));
    // restore a closed header: readable as final again, by the raw reader of other programs
    let proof = pv::acc::prove(sha, &hashes, 5).unwrap();
    let cu = env.send(&[client::restore_header(relayer, blks[4].bytes, count, proof.clone())], &[]).unwrap();
    println!("restore_header: {cu} CU");
    let data = env.svm.get_account(&client::header_addr(&blks[4].hash)).unwrap().data;
    assert_eq!(lightclient::read_final_header(&data), Some((blks[4].hash, blks[4].bytes)));
    assert!(env.send(&[client::restore_header(relayer, blks[4].bytes, count, proof)], &[]).is_err(), "already there");
    // a header that never was final cannot be restored
    let fake = child(&p, &blks[3], &Work::Fallback, ChildOpts { dt: 7_000, ..Default::default() });
    let fp = pv::acc::prove(sha, &hashes, 5).unwrap();
    assert!(env.send(&[client::restore_header(relayer, fake.bytes, count, fp)], &[]).is_err());
}

/// R5 CU at scale: an accumulator of 2^20 - 1 final headers (20 peaks, the longest
/// proofs below 2^20; about 2 years of 60 s blocks), written straight into the account.
/// prove_header checks no proof of work, so the leaf is a header-shaped 256 bytes at the
/// right height.
#[test]
fn prove_header_cu_at_a_million_headers() {
    use anchor_lang::Discriminator;
    let p = params(3);
    let g = genesis(0x1e100000);
    let mut env = Env::new(p, &g);
    let n: u64 = (1 << 20) - 1;
    let start: u64 = 100;
    let mut base: Vec<[u8; 32]> = (0..n).map(|k| sha(&[&k.to_le_bytes()])).collect();
    for &i in &[0u64, 777_777, n - 1] {
        let mut hdr = g.bytes;
        hdr[4..12].copy_from_slice(&(start + i).to_le_bytes());
        let hash = pv::blake3::hash(&hdr);
        let keep = base[i as usize];
        base[i as usize] = hash;
        let mut slots = [[0u8; 32]; 64];
        for (k, h) in base.iter().enumerate() {
            pv::acc::append(sha, k as u64, &mut slots, h);
        }
        let root = pv::acc::root_of_slots(sha, n, &slots);
        let mut data = vec![0u8; lightclient::acc_layout::LEN];
        data[..8].copy_from_slice(lightclient::Accumulator::DISCRIMINATOR);
        data[8..16].copy_from_slice(&start.to_le_bytes());
        data[16..24].copy_from_slice(&n.to_le_bytes());
        data[24..56].copy_from_slice(base.last().unwrap());
        for b in 0..64 {
            data[56 + 32 * b..88 + 32 * b].copy_from_slice(&slots[b]);
        }
        let ro = lightclient::acc_layout::ROOTS + 32 * (n as usize % lightclient::ACC_ROOTS);
        data[ro..ro + 32].copy_from_slice(&root);
        data[lightclient::acc_layout::LEN - 8] = lightclient::acc_pda().1;
        let mut acct = env.svm.get_account(&client::acc_addr()).unwrap();
        acct.data = data;
        env.svm.set_account(client::acc_addr(), acct).unwrap();
        let proof = pv::acc::prove_short(sha, &base, i).unwrap();
        let (cu, ret) = env.send_return(&[client::prove_header(hdr, n, proof.clone())]).unwrap();
        assert_eq!(&ret[..32], &hash);
        assert_eq!(u64::from_le_bytes(ret[32..40].try_into().unwrap()), start + i);
        println!("prove_header at {n} leaves, leaf {i}: proof {} bytes, {cu} CU", proof.len());
        assert!(cu < 80_000, "{cu}");
        base[i as usize] = keep;
    }
}
