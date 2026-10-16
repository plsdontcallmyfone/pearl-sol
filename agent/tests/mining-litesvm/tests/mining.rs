mod common;
use anchor_lang::{InstructionData, ToAccountMetas};
use common::*;
use mining::reward;
use solana_sdk::{
    instruction::Instruction,
    pubkey::Pubkey,
    signature::{Keypair, Signer},
};

const W: u64 = 2 * SOL;

// MiningError codes
const EPOCH_NOT_OVER: u32 = 6001;
const EPOCH_OVER: u32 = 6002;
const JOB_NOT_OPEN: u32 = 6003;
const NONCE_RANGE: u32 = 6004;
const REPLAYED: u32 = 6005;
const BAD_CONFIG: u32 = 6000;
const MISSING_SIG: u32 = 7003;

fn mining_env(epoch_slots: u64, emission_bps: u16) -> Env {
    Env::with_mining(mining::ConfigParams {
        epoch_slots: Some(epoch_slots),
        emission_bps: Some(emission_bps),
        max_nonce: Some(64),
        ..Default::default()
    })
}

#[test]
fn share_accepted_counted_and_cu() {
    let mut env = mining_env(1000, 42);
    let poster = env.user(10 * SOL);
    let miner = env.user(SOL);
    let p = SOL / 2;
    let jk = env.post_simple(&poster, 1, W, p, 500);

    let r = env.submit_share(&miner, jk, 0);
    let cu_first = r.as_ref().map(|m| m.compute_units_consumed).unwrap_or(0);
    assert!(r.is_ok(), "{}", logs(&r));
    let r = env.submit_share(&miner, jk, 1);
    assert!(r.is_ok(), "{}", logs(&r));
    let cu_next = r.unwrap().compute_units_consumed;
    println!("submit_share CU: first share (creates 4 accounts) = {cu_first}, later share = {cu_next}");

    let sh: mining::Shares = env.account(&shares_pda(0, &jk, &miner.pubkey()));
    assert_eq!((sh.job, sh.miner, sh.count), (jk, miner.pubkey(), 2));
    let je: mining::JobEpoch = env.account(&job_epoch_pda(0, &jk));
    assert_eq!((je.total_shares, je.fee_weight), (2, p));
    let e = env.epoch(0);
    assert_eq!((e.total_fee, e.jobs_active), (p, 1));
    let js: mining::JobStats = env.account(&job_stats_pda(&jk));
    assert_eq!(js.cap, p * 8 / 10);
    let nb: mining::NonceBitmap = env.account(&nonces_pda(&jk, &miner.pubkey()));
    assert_eq!((nb.capacity, nb.used, nb.bits[0]), (64, 2, 0b11));
}

#[test]
fn bad_signatures_rejected() {
    let mut env = mining_env(1000, 42);
    let poster = env.user(10 * SOL);
    let miner = env.user(SOL);
    let thief = env.user(SOL);
    let jk = env.post_simple(&poster, 1, W, SOL / 2, 500);

    // signed by a key that is not an attester
    let fake = Keypair::new();
    let ixs = env.share_ixs(&fake, jk, miner.pubkey(), 0);
    assert_eq!(err_code(&env.send(&ixs, &miner, &[])), Some(MISSING_SIG));

    // attester signed nonce 1, miner submits nonce 0
    let att = env.attester.insecure_clone();
    let msg = mining::share_message(&jk, &miner.pubkey(), 1);
    let ixs = vec![ed25519_ix(&att, &msg), env.submit_share_ix(jk, miner.pubkey(), 0)];
    assert_eq!(err_code(&env.send(&ixs, &miner, &[])), Some(MISSING_SIG));

    // stolen share: attester signed for miner, thief submits it as their own
    let msg = mining::share_message(&jk, &miner.pubkey(), 0);
    let ixs = vec![ed25519_ix(&att, &msg), env.submit_share_ix(jk, thief.pubkey(), 0)];
    assert_eq!(err_code(&env.send(&ixs, &thief, &[])), Some(MISSING_SIG));

    // no ed25519 instruction
    let ixs = vec![env.submit_share_ix(jk, miner.pubkey(), 0)];
    assert_eq!(err_code(&env.send(&ixs, &miner, &[])), Some(MISSING_SIG));

    // corrupted signature bytes: the precompile fails the whole transaction
    let mut ixs = env.share_ixs(&att, jk, miner.pubkey(), 0);
    ixs[0].data[16 + 32] ^= 0xFF; // first signature byte (offsets 16, pubkey 32, then signature)
    let r = env.send(&ixs, &miner, &[]);
    assert_eq!(err_at(&r).map(|x| x.0), Some(0), "rejected by the precompile (ix 0)");

    // valid precompile ix whose offsets point at instruction 0 explicitly instead of
    // u16::MAX: the precompile accepts it, the program must ignore it
    let mut ixs = env.share_ixs(&att, jk, miner.pubkey(), 0);
    for f in [1usize, 3, 6] {
        let at = 2 + 2 * f;
        ixs[0].data[at..at + 2].copy_from_slice(&0u16.to_le_bytes());
    }
    assert_eq!(err_code(&env.send(&ixs, &miner, &[])), Some(MISSING_SIG));

    // finally the honest one works
    env.submit_share(&miner, jk, 0).unwrap();
}

#[test]
fn replay_and_nonce_cap() {
    let mut env = mining_env(1000, 42);
    let poster = env.user(10 * SOL);
    let miner = env.user(SOL);
    let other = env.user(SOL);
    let jk = env.post_simple(&poster, 1, W, SOL / 2, 5_000);
    env.submit_share(&miner, jk, 5).unwrap();
    assert_eq!(err_code(&env.submit_share(&miner, jk, 5)), Some(REPLAYED));
    // same nonce, different miner: a different seed, so a different share
    env.submit_share(&other, jk, 5).unwrap();
    // replay protection outlives the epoch
    env.warp(env.epoch(0).end_slot);
    let admin = env.admin.insecure_clone();
    env.advance_epoch(&admin).unwrap();
    assert_eq!(err_code(&env.submit_share(&miner, jk, 5)), Some(REPLAYED));
    env.submit_share(&miner, jk, 63).unwrap();
    assert_eq!(err_code(&env.submit_share(&miner, jk, 64)), Some(NONCE_RANGE));
}

#[test]
fn shares_only_on_open_jobs() {
    let mut env = mining_env(1000, 42);
    let poster = env.user(10 * SOL);
    let miner = env.user(SOL);
    let jk = env.post_simple(&poster, 1, W, SOL / 2, 500);
    env.submit_result(&miner, jk, [1; 32]).unwrap();
    assert_eq!(err_code(&env.submit_share(&miner, jk, 0)), Some(JOB_NOT_OPEN));

    let jk2 = env.post_simple(&poster, 2, W, SOL / 2, 50);
    env.warp(env.slot() + 51);
    assert_eq!(err_code(&env.submit_share(&miner, jk2, 0)), Some(JOB_NOT_OPEN));
}

/// Two jobs, two miners, one funded epoch: every lamport is checked against the pure
/// reward functions, and the pool's accounting closes.
#[test]
fn epoch_rollover_and_claim_math() {
    let slots = 200;
    let bps = 100; // 1% per epoch, so numbers are visible
    let mut env = mining_env(slots, bps);
    let admin = env.admin.insecure_clone();
    assert_eq!(env.epoch(0).budget, 0, "pool empty at init");

    env.fund_pool(1_000 * SOL);
    // early advance refused
    assert_eq!(err_code(&env.advance_epoch(&admin)), Some(EPOCH_NOT_OVER));
    env.warp(env.epoch(0).end_slot);
    let free = env.pool_free();
    env.advance_epoch(&admin).unwrap();
    let e1 = env.epoch(1);
    let budget = reward::epoch_budget(free, bps);
    assert_eq!(e1.budget, budget);
    assert_eq!(budget, 10 * SOL);
    assert_eq!(env.mining_cfg().outstanding, budget, "epoch 0 had no jobs and was released");

    let poster = env.user(100 * SOL);
    let (p1, p2) = (SOL, 30 * SOL);
    let j1 = env.post_simple(&poster, 1, W, p1, 10_000);
    let j2 = env.post_simple(&poster, 2, W, p2, 10_000);
    let a = env.user(SOL);
    let b = env.user(SOL);
    for n in 0..3 {
        env.submit_share(&a, j1, n).unwrap();
    }
    env.submit_share(&b, j1, 0).unwrap();
    env.submit_share(&a, j2, 0).unwrap();

    // P paid after the snapshot does not change this epoch's budget
    assert_eq!(env.epoch(1).budget, budget);
    assert_eq!(env.epoch(1).total_fee, p1 + p2);

    assert_eq!(err_code(&env.claim(&a, 1, j1)), Some(EPOCH_NOT_OVER));
    env.warp(e1.end_slot);
    assert_eq!(err_code(&env.submit_share(&a, j1, 9)), Some(EPOCH_OVER));
    let free2 = env.pool_free();
    env.advance_epoch(&admin).unwrap();
    let budget2 = reward::epoch_budget(free2, bps);
    assert_eq!(env.epoch(2).budget, budget2);

    // expected
    let alpha = env.mining_cfg().alpha_bps;
    let s1 = reward::job_subsidy(budget, p1, p1 + p2, reward::job_cap(p1, alpha));
    let s2 = reward::job_subsidy(budget, p2, p1 + p2, reward::job_cap(p2, alpha));
    // job 1: E share = 10 SOL / 31 = 0.3225 SOL, cap 0.8 SOL -> E share binds
    // job 2: E share = 9.677 SOL, cap 24 SOL -> E share binds
    assert_eq!(s1, budget * p1 / (p1 + p2));
    let a1 = reward::miner_reward(s1, 3, 4, 0, 0);
    let b1 = s1 - a1;

    let pool0 = env.balance(&pool());
    let (ab, bb) = (env.balance(&a.pubkey()), env.balance(&b.pubkey()));
    let shares_rent_a1 = env.balance(&shares_pda(1, &j1, &a.pubkey()));
    let shares_rent_a2 = env.balance(&shares_pda(1, &j2, &a.pubkey()));
    let shares_rent_b1 = env.balance(&shares_pda(1, &j1, &b.pubkey()));
    let r = env.claim(&a, 1, j1);
    assert!(r.is_ok(), "{}", logs(&r));
    env.claim(&b, 1, j1).unwrap();
    env.claim(&a, 1, j2).unwrap();
    assert_eq!(env.balance(&a.pubkey()), ab + a1 + s2 + shares_rent_a1 + shares_rent_a2 - 2 * 5000);
    assert_eq!(env.balance(&b.pubkey()), bb + b1 + shares_rent_b1 - 5000);
    assert_eq!(env.balance(&pool()), pool0 - s1 - s2, "pool paid exactly S1 + S2");

    // double claim impossible (Shares closed)
    assert!(env.claim(&a, 1, j1).is_err());

    // epoch 1 fully settled: its unallocated budget is released, only epoch 2 is owed
    let e1 = env.epoch(1);
    assert!(e1.released);
    assert_eq!(e1.allocated, s1 + s2);
    assert_eq!(env.mining_cfg().outstanding, budget2);
    println!("epoch 1: E = {budget}, S1 = {s1}, S2 = {s2}, a = {a1}+{s2}, b = {b1}");
}

/// SPEC 3.5 on chain: an agent posts a job, mines it alone in every epoch, delivers
/// and accepts its own result. With a pool so large the E share never binds, it
/// collects exactly alpha x P over the job's life and loses (1 - alpha) x P.
#[test]
fn self_dealer_loses_one_minus_alpha_p_on_chain() {
    let slots = 100;
    let mut env = mining_env(slots, 42);
    let admin = env.admin.insecure_clone();
    env.fund_pool(1_000_000 * SOL);
    env.warp(env.epoch(0).end_slot);
    env.advance_epoch(&admin).unwrap();

    let dealer = env.user(100 * SOL);
    let p = 10 * SOL;
    let jk = env.post_simple(&dealer, 1, W, p, 10_000);

    let mut received = 0u64;
    let mut nonce = 0;
    for _ in 0..3 {
        let e = env.mining_cfg().current_epoch;
        for _ in 0..4 {
            env.submit_share(&dealer, jk, nonce).unwrap();
            nonce += 1;
        }
        env.warp(env.epoch(e).end_slot);
        env.advance_epoch(&admin).unwrap();
        let pool0 = env.balance(&pool());
        env.claim(&dealer, e, jk).unwrap();
        received += pool0 - env.balance(&pool());
    }
    let alpha = env.mining_cfg().alpha_bps;
    let cap = reward::job_cap(p, alpha);
    assert_eq!(received, cap, "lifetime subsidy is exactly alpha x P");

    // dealer delivers to itself and accepts: W comes back
    env.submit_result(&dealer, jk, [1; 32]).unwrap();
    let ix = Instruction {
        program_id: jobs::ID,
        accounts: jobs::accounts::Accept { poster: dealer.pubkey(), job: jk, deliverer: dealer.pubkey() }
            .to_account_metas(None),
        data: jobs::instruction::Accept {}.data(),
    };
    let before = env.balance(&dealer.pubkey());
    env.send(&[ix], &dealer, &[]).unwrap();
    assert_eq!(env.balance(&dealer.pubkey()), before + W - 5000);

    let net = reward::self_dealer_net(p, received);
    assert_eq!(net, -((p - cap) as i128));
    assert_eq!(net, -(2 * SOL as i128), "loses (1 - 0.8) x 10 SOL");
}

#[test]
fn admin_edits_mining_config() {
    let mut env = mining_env(100, 42);
    let admin = env.admin.insecure_clone();
    let upd = |who: Pubkey, params: mining::ConfigParams| Instruction {
        program_id: mining::ID,
        accounts: mining::accounts::UpdateConfig { admin: who, config: mining_config() }.to_account_metas(None),
        data: mining::instruction::UpdateConfig { params }.data(),
    };
    let s = env.user(SOL);
    let p = mining::ConfigParams { emission_bps: Some(10), ..Default::default() };
    assert!(env.send(&[upd(s.pubkey(), p.clone())], &s, &[]).is_err());
    env.send(&[upd(admin.pubkey(), p)], &admin, &[]).unwrap();
    assert_eq!(env.mining_cfg().emission_bps, 10);

    let p = mining::ConfigParams { alpha_bps: Some(10_000), ..Default::default() };
    assert_eq!(err_code(&env.send(&[upd(admin.pubkey(), p)], &admin, &[])), Some(BAD_CONFIG), "alpha must be < 1");

    // rotate the attester set: old attester's shares stop counting
    let new_att = Keypair::new();
    let p = mining::ConfigParams {
        attesters: Some(vec![new_att.pubkey()]),
        epoch_slots: Some(500),
        alpha_bps: Some(5000),
        max_nonce: Some(8),
        ..Default::default()
    };
    env.send(&[upd(admin.pubkey(), p)], &admin, &[]).unwrap();
    let poster = env.user(10 * SOL);
    let miner = env.user(SOL);
    let jk = env.post_simple(&poster, 1, W, SOL, 500);
    assert_eq!(err_code(&env.submit_share(&miner, jk, 0)), Some(MISSING_SIG));
    let ixs = env.share_ixs(&new_att, jk, miner.pubkey(), 0);
    env.send(&ixs, &miner, &[]).unwrap();
    assert_eq!(err_code(&{
        let ixs = env.share_ixs(&new_att, jk, miner.pubkey(), 8);
        env.send(&ixs, &miner, &[])
    }), Some(NONCE_RANGE));
    let js: mining::JobStats = env.account(&job_stats_pda(&jk));
    assert_eq!(js.cap, SOL / 2, "cap snapshots the alpha in force at the job's first share");
}
