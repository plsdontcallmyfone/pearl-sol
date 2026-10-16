mod common;
use anchor_lang::{InstructionData, ToAccountMetas};
use common::*;
use jobs::{Job, JobStatus};
use solana_sdk::{
    hash::hashv,
    instruction::Instruction,
    pubkey::Pubkey,
    signature::{Keypair, Signer},
};

const W: u64 = 2 * SOL;
const P: u64 = SOL / 2;

// JobsError codes
const BAD_CREATED_SLOT: u32 = 6000;
const BAD_STATUS: u32 = 6006;
const PAST_DEADLINE: u32 = 6007;
const NOT_EXPIRED: u32 = 6008;
const MISSING_SIG: u32 = 7003;

#[test]
fn post_job_escrows_and_builds_header() {
    let mut env = Env::new();
    let poster = env.user(10 * SOL);
    let pool_before = env.balance(&pool());
    let created = env.slot() - 5;
    let params = job_params(42, W, P, created, env.slot() + 500);
    env.post_job(&poster, params.clone()).unwrap();

    let jk = job_pda(&poster.pubkey(), 42);
    let j: Job = env.account(&jk);
    let rent = env.rent(env.svm.get_account(&jk).unwrap().data.len());
    assert_eq!(env.balance(&jk), rent + W, "W escrowed in the job PDA");
    assert_eq!(env.balance(&pool()), pool_before + P, "P moved to mining_pool");
    assert_eq!(j.status, JobStatus::Open);
    assert_eq!(j.hash_a, [0xAA; 32]);
    assert_eq!(j.config, [7u8; 52]);

    // terms_hash, independently recomputed
    let th = hashv(&[
        b"pearl-sol:terms:v1",
        poster.pubkey().as_ref(),
        &42u64.to_le_bytes(),
        &64u32.to_le_bytes(),
        &64u32.to_le_bytes(),
        &1024u32.to_le_bytes(),
        &W.to_le_bytes(),
        &P.to_le_bytes(),
        &params.deadline_slot.to_le_bytes(),
        &params.share_target.to_le_bytes(),
    ])
    .to_bytes();
    assert_eq!(j.terms_hash, th);
    let mut h = Vec::new();
    h.extend_from_slice(jk.as_ref());
    h.extend_from_slice(&th);
    h.extend_from_slice(&created.to_le_bytes());
    h.extend_from_slice(&params.share_target.to_le_bytes());
    assert_eq!(h.len(), 76);
    assert_eq!(j.header.to_vec(), h);
}

#[test]
fn created_slot_must_be_recent() {
    let mut env = Env::new();
    let poster = env.user(10 * SOL);
    let now = env.slot();
    let r = env.post_job(&poster, job_params(1, W, P, now + 1, now + 500));
    assert_eq!(err_code(&r), Some(BAD_CREATED_SLOT), "future slot");
    let r = env.post_job(&poster, job_params(1, W, P, now - 151, now + 500));
    assert_eq!(err_code(&r), Some(BAD_CREATED_SLOT), "stale slot");
    env.post_job(&poster, job_params(1, W, P, now - 150, now + 500)).unwrap();
}

#[test]
fn lifecycle_attester_complete() {
    let mut env = Env::new();
    let poster = env.user(10 * SOL);
    let worker = env.user(SOL);
    let jk = env.post_simple(&poster, 1, W, P, 500);

    // cannot complete before delivery
    let att = env.attester.insecure_clone();
    let msg = jobs::result_message(&jk, &Pubkey::default(), &[0; 32]);
    let r = env.send(&[ed25519_ix(&att, &msg), env.complete_ix(jk, Pubkey::default())], &worker, &[]);
    assert!(r.is_err());

    let rh = [0x5C; 32];
    env.submit_result(&worker, jk, rh).unwrap();
    let j: Job = env.account(&jk);
    assert_eq!(j.status, JobStatus::Delivered);
    assert_eq!(j.deliverer, worker.pubkey());

    // second submitter is refused
    let other = env.user(SOL);
    assert_eq!(err_code(&env.submit_result(&other, jk, [1; 32])), Some(BAD_STATUS));

    // a non-attester signature is refused
    let fake = Keypair::new();
    let good_msg = jobs::result_message(&jk, &worker.pubkey(), &rh);
    let r = env.send(&[ed25519_ix(&fake, &good_msg), env.complete_ix(jk, worker.pubkey())], &worker, &[]);
    assert_eq!(err_code(&r), Some(MISSING_SIG));

    // attester signature over a different result hash is refused
    let bad_msg = jobs::result_message(&jk, &worker.pubkey(), &[0x5D; 32]);
    let r = env.send(&[ed25519_ix(&att, &bad_msg), env.complete_ix(jk, worker.pubkey())], &worker, &[]);
    assert_eq!(err_code(&r), Some(MISSING_SIG));

    // no ed25519 instruction at all
    let r = env.send(&[env.complete_ix(jk, worker.pubkey())], &worker, &[]);
    assert_eq!(err_code(&r), Some(MISSING_SIG));

    // the real one; sent by an unrelated cranker
    let cranker = env.user(SOL);
    let before = env.balance(&worker.pubkey());
    let r = env.send(&[ed25519_ix(&att, &good_msg), env.complete_ix(jk, worker.pubkey())], &cranker, &[]);
    assert!(r.is_ok(), "{}", logs(&r));
    assert_eq!(env.balance(&worker.pubkey()), before + W);
    let j: Job = env.account(&jk);
    assert_eq!(j.status, JobStatus::Completed);

    // paying twice is impossible
    let r = env.send(&[ed25519_ix(&att, &good_msg), env.complete_ix(jk, worker.pubkey())], &cranker, &[]);
    assert_eq!(err_code(&r), Some(BAD_STATUS));

    // poster closes the account and gets the rent back
    let rent = env.balance(&jk);
    let pb = env.balance(&poster.pubkey());
    let ix = Instruction {
        program_id: jobs::ID,
        accounts: jobs::accounts::CloseJob { poster: poster.pubkey(), job: jk }.to_account_metas(None),
        data: jobs::instruction::CloseJob {}.data(),
    };
    env.send(&[ix], &poster, &[]).unwrap();
    assert!(!env.exists(&jk));
    assert_eq!(env.balance(&poster.pubkey()), pb + rent - 5000);
}

#[test]
fn tampered_precompile_signature_fails_tx() {
    let mut env = Env::new();
    let poster = env.user(10 * SOL);
    let worker = env.user(SOL);
    let jk = env.post_simple(&poster, 1, W, P, 500);
    let rh = [3; 32];
    env.submit_result(&worker, jk, rh).unwrap();
    let att = env.attester.insecure_clone();
    let mut ed = ed25519_ix(&att, &jobs::result_message(&jk, &worker.pubkey(), &rh));
    // signature starts at 16 + 32 (offsets header, then pubkey) in the layout we build
    let n = ed.data.len();
    ed.data[n - 1] ^= 1; // flip a message bit: precompile now rejects the whole tx
    let r = env.send(&[ed, env.complete_ix(jk, worker.pubkey())], &worker, &[]);
    assert!(r.is_err());
    assert_eq!(err_at(&r).map(|x| x.0), Some(0), "rejected by the precompile (ix 0), not the program");
}

#[test]
fn lifecycle_poster_accept() {
    let mut env = Env::new();
    let poster = env.user(10 * SOL);
    let worker = env.user(SOL);
    let jk = env.post_simple(&poster, 7, W, P, 500);
    env.submit_result(&worker, jk, [9; 32]).unwrap();
    let accept = |_env: &Env, who: &Pubkey| Instruction {
        program_id: jobs::ID,
        accounts: jobs::accounts::Accept { poster: *who, job: jk, deliverer: worker.pubkey() }.to_account_metas(None),
        data: jobs::instruction::Accept {}.data(),
    };
    // a stranger cannot accept
    let s = env.user(SOL);
    let ix = accept(&env, &s.pubkey());
    assert!(env.send(&[ix], &s, &[]).is_err());
    let before = env.balance(&worker.pubkey());
    let ix = accept(&env, &poster.pubkey());
    env.send(&[ix], &poster, &[]).unwrap();
    assert_eq!(env.balance(&worker.pubkey()), before + W);
    assert_eq!(env.account::<Job>(&jk).status, JobStatus::Completed);
}

#[test]
fn dispute_then_admin_refund() {
    let mut env = Env::new();
    let poster = env.user(10 * SOL);
    let worker = env.user(SOL);
    let jk = env.post_simple(&poster, 8, W, P, 500);
    env.submit_result(&worker, jk, [9; 32]).unwrap();
    let ix = Instruction {
        program_id: jobs::ID,
        accounts: jobs::accounts::Dispute { poster: poster.pubkey(), job: jk }.to_account_metas(None),
        data: jobs::instruction::Dispute {}.data(),
    };
    env.send(&[ix], &poster, &[]).unwrap();
    assert_eq!(env.account::<Job>(&jk).status, JobStatus::Disputed);

    let resolve = |admin: Pubkey, to_deliverer: bool| Instruction {
        program_id: jobs::ID,
        accounts: jobs::accounts::Resolve {
            admin,
            config: jobs_config(),
            job: jk,
            poster: poster.pubkey(),
            deliverer: worker.pubkey(),
        }
        .to_account_metas(None),
        data: jobs::instruction::Resolve { to_deliverer }.data(),
    };
    // only admin
    assert!(env.send(&[resolve(poster.pubkey(), false)], &poster, &[]).is_err());
    let admin = env.admin.insecure_clone();
    let pb = env.balance(&poster.pubkey());
    env.send(&[resolve(admin.pubkey(), false)], &admin, &[]).unwrap();
    assert_eq!(env.balance(&poster.pubkey()), pb + W);
    assert_eq!(env.account::<Job>(&jk).status, JobStatus::Refunded);
}

#[test]
fn dispute_then_attester_completes() {
    let mut env = Env::new();
    let poster = env.user(10 * SOL);
    let worker = env.user(SOL);
    let jk = env.post_simple(&poster, 9, W, P, 500);
    let rh = [4; 32];
    env.submit_result(&worker, jk, rh).unwrap();
    let ix = Instruction {
        program_id: jobs::ID,
        accounts: jobs::accounts::Dispute { poster: poster.pubkey(), job: jk }.to_account_metas(None),
        data: jobs::instruction::Dispute {}.data(),
    };
    env.send(&[ix], &poster, &[]).unwrap();
    let att = env.attester.insecure_clone();
    let msg = jobs::result_message(&jk, &worker.pubkey(), &rh);
    let before = env.balance(&worker.pubkey());
    let cranker = env.user(SOL);
    env.send(&[ed25519_ix(&att, &msg), env.complete_ix(jk, worker.pubkey())], &cranker, &[])
        .unwrap();
    assert_eq!(env.balance(&worker.pubkey()), before + W);
    assert_eq!(env.account::<Job>(&jk).status, JobStatus::Completed);
}

#[test]
fn expiry_refunds_poster() {
    let mut env = Env::new();
    let poster = env.user(10 * SOL);
    let worker = env.user(SOL);
    let jk = env.post_simple(&poster, 3, W, P, 100);
    let deadline = env.account::<Job>(&jk).deadline_slot;
    let expire = Instruction {
        program_id: jobs::ID,
        accounts: jobs::accounts::Expire { job: jk, poster: poster.pubkey() }.to_account_metas(None),
        data: jobs::instruction::Expire {}.data(),
    };
    let cranker = env.user(SOL);
    assert_eq!(err_code(&env.send(&[expire.clone()], &cranker, &[])), Some(NOT_EXPIRED));
    env.warp(deadline + 1);
    assert_eq!(err_code(&env.submit_result(&worker, jk, [1; 32])), Some(PAST_DEADLINE));
    let pb = env.balance(&poster.pubkey());
    let pool_b = env.balance(&pool());
    env.send(&[expire.clone()], &cranker, &[]).unwrap();
    assert_eq!(env.balance(&poster.pubkey()), pb + W, "W refunded");
    assert_eq!(env.balance(&pool()), pool_b, "P stays in the pool");
    assert_eq!(env.account::<Job>(&jk).status, JobStatus::Expired);
    assert_eq!(err_code(&env.send(&[expire], &cranker, &[])), Some(BAD_STATUS));
}

#[test]
fn admin_edits_jobs_config() {
    let mut env = Env::new();
    let admin = env.admin.insecure_clone();
    let upd = |who: Pubkey| Instruction {
        program_id: jobs::ID,
        accounts: jobs::accounts::UpdateConfig { admin: who, config: jobs_config() }.to_account_metas(None),
        data: jobs::instruction::UpdateConfig {
            params: jobs::ConfigParams { min_protocol_fee: Some(SOL), ..Default::default() },
        }
        .data(),
    };
    let s = env.user(SOL);
    assert!(env.send(&[upd(s.pubkey())], &s, &[]).is_err());
    env.send(&[upd(admin.pubkey())], &admin, &[]).unwrap();
    let poster = env.user(10 * SOL);
    let now = env.slot();
    let r = env.post_job(&poster, job_params(1, W, P, now, now + 100));
    assert_eq!(err_code(&r), Some(6004), "fee below new minimum");
}
