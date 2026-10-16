//! Useful-PoW mining on posted jobs (SPEC 3.4, 3.5).
//!
//! M1 is attested: the attester runs Pearl's plain verifier off chain and signs
//! `b"pearl-sol:share:v1" || job [32] || miner [32] || nonce u64 LE`; `submit_share`
//! checks that signature through the ed25519 precompile in the same transaction.
//!
//! Accounts:
//! - `config` `["config"]`: admin, attester set, epoch_slots, emission_bps,
//!   alpha_bps, max_nonce (all admin-editable), current epoch, outstanding.
//! - `mining_pool` `["mining_pool"]`: holds the pool lamports (program-owned, so
//!   payouts are direct lamport moves). Protocol fees and `fund_mining` land here.
//! - `epoch` `["epoch", index u64]`: budget E snapshotted at epoch start.
//! - `job_stats` `["job_stats", job]`: the job's fee P and lifetime cap alpha x P.
//! - `job_epoch` `["job_epoch", index, job]`: shares on this job in this epoch.
//! - `shares` `["shares", index, job, miner]`: Shares { job, miner, count } per epoch.
//! - `nonces` `["nonces", job, miner]`: replay bitmap, one bit per nonce.
//!
//! Replay protection is a bitmap per (job, miner), not a PDA per nonce: one account
//! and one rent deposit per miner per job instead of one per share, O(1) check, and
//! the size is small and bounded because `max_nonce` is small by design (re-grinding
//! nonces on the same job redoes the multiply and is not new useful work). The bitmap
//! is sized from `max_nonce` when created; a later raise of `max_nonce` does not
//! grow existing bitmaps (accepted nonce < min(max_nonce, bitmap capacity)).
//!
//! `outstanding` = lamports promised to epoch budgets and not yet paid or released.
//! A new budget is a fraction of `pool - rent - outstanding`, so the pool can always
//! pay every claim.

use anchor_lang::prelude::*;
use jobs::{sigcheck, Job, JobStatus};

pub mod reward;

declare_id!("3GrrgQMSHC3PjkW6UmvQJPGK1WBvhxiz3tkigafj3GhP");

pub const CONFIG_SEED: &[u8] = b"config";
pub const POOL_SEED: &[u8] = jobs::MINING_POOL_SEED;
pub const EPOCH_SEED: &[u8] = b"epoch";
pub const JOB_STATS_SEED: &[u8] = b"job_stats";
pub const JOB_EPOCH_SEED: &[u8] = b"job_epoch";
pub const SHARES_SEED: &[u8] = b"shares";
pub const NONCES_SEED: &[u8] = b"nonces";
pub const SHARE_DOMAIN: &[u8] = b"pearl-sol:share:v1";
pub const MAX_ATTESTERS: usize = 8;
pub const MAX_NONCE_CAP: u32 = 8192;

#[program]
pub mod mining {
    use super::*;

    /// One-time setup: config, pool and epoch 0. The signer becomes admin.
    pub fn initialize(ctx: Context<Initialize>, params: ConfigParams) -> Result<()> {
        let now = Clock::get()?.slot;
        let c = &mut ctx.accounts.config;
        c.admin = ctx.accounts.admin.key();
        c.attesters = vec![];
        c.threshold = 1;
        c.epoch_slots = 9000;
        c.emission_bps = 42;
        c.alpha_bps = 8000;
        c.max_nonce = 256;
        c.current_epoch = 0;
        c.outstanding = 0;
        c.bump = ctx.bumps.config;
        apply_config(c, params)?;
        ctx.accounts.mining_pool.bump = ctx.bumps.mining_pool;

        let pool_free = pool_free(&ctx.accounts.mining_pool.to_account_info(), 0)?;
        let e = &mut ctx.accounts.epoch;
        e.index = 0;
        e.start_slot = now;
        e.end_slot = now + c.epoch_slots;
        e.budget = reward::epoch_budget(pool_free, c.emission_bps);
        e.bump = ctx.bumps.epoch;
        c.outstanding = e.budget;
        emit!(EpochStarted { index: 0, start_slot: now, budget: e.budget });
        Ok(())
    }

    pub fn update_config(ctx: Context<UpdateConfig>, params: ConfigParams) -> Result<()> {
        apply_config(&mut ctx.accounts.config, params)
    }

    /// Permissionless: close the current epoch once its slots are over and start the
    /// next one, snapshotting E = (pool - rent - outstanding) x emission_bps / 10_000.
    pub fn advance_epoch(ctx: Context<AdvanceEpoch>) -> Result<()> {
        let now = Clock::get()?.slot;
        let c = &mut ctx.accounts.config;
        let cur = &mut ctx.accounts.current_epoch;
        require!(now >= cur.end_slot, MiningError::EpochNotOver);
        // An epoch nobody mined in releases its whole budget right away.
        if cur.jobs_active == 0 && !cur.released {
            c.outstanding = c.outstanding.saturating_sub(cur.budget);
            cur.released = true;
        }
        let free = pool_free(&ctx.accounts.mining_pool.to_account_info(), c.outstanding)?;
        let e = &mut ctx.accounts.next_epoch;
        e.index = cur.index + 1;
        e.start_slot = now;
        e.end_slot = now + c.epoch_slots;
        e.budget = reward::epoch_budget(free, c.emission_bps);
        e.bump = ctx.bumps.next_epoch;
        c.current_epoch = e.index;
        c.outstanding = c.outstanding.checked_add(e.budget).ok_or(MiningError::Overflow)?;
        emit!(EpochStarted { index: e.index, start_slot: now, budget: e.budget });
        Ok(())
    }

    /// Record one attested share for (job, miner = signer, nonce) in the current epoch.
    pub fn submit_share(ctx: Context<SubmitShare>, nonce: u64) -> Result<()> {
        let now = Clock::get()?.slot;
        let c = &ctx.accounts.config;
        let job = &ctx.accounts.job;
        let job_key = job.key();
        let miner = ctx.accounts.miner.key();
        let epoch = &mut ctx.accounts.epoch;
        require!(now < epoch.end_slot, MiningError::EpochOver);
        require!(job.status == JobStatus::Open, MiningError::JobNotOpen);
        require!(now <= job.deadline_slot, MiningError::JobNotOpen);
        require!(nonce < c.max_nonce as u64, MiningError::NonceOutOfRange);

        let msg = share_message(&job_key, &miner, nonce);
        sigcheck::require_signed(&ctx.accounts.instructions, &msg, &c.attesters, c.threshold)?;

        // replay bitmap
        let nb = &mut ctx.accounts.nonces;
        if nb.job == Pubkey::default() {
            let cap = c.max_nonce.min(MAX_NONCE_CAP);
            nb.job = job_key;
            nb.miner = miner;
            nb.capacity = cap;
            nb.bits = vec![0u8; cap.div_ceil(8) as usize];
            nb.bump = ctx.bumps.nonces;
        }
        require!(nonce < nb.capacity as u64, MiningError::NonceOutOfRange);
        let (byte, bit) = ((nonce / 8) as usize, 1u8 << (nonce % 8));
        require!(nb.bits[byte] & bit == 0, MiningError::ShareReplayed);
        nb.bits[byte] |= bit;
        nb.used = nb.used.saturating_add(1);

        let js = &mut ctx.accounts.job_stats;
        if js.job == Pubkey::default() {
            js.job = job_key;
            js.protocol_fee = job.protocol_fee;
            js.cap = reward::job_cap(job.protocol_fee, c.alpha_bps);
            js.bump = ctx.bumps.job_stats;
        }

        let je = &mut ctx.accounts.job_epoch;
        if je.job == Pubkey::default() {
            je.job = job_key;
            je.epoch = epoch.index;
            je.fee_weight = js.protocol_fee;
            je.bump = ctx.bumps.job_epoch;
            epoch.total_fee = epoch.total_fee.checked_add(js.protocol_fee).ok_or(MiningError::Overflow)?;
            epoch.jobs_active += 1;
        }
        je.total_shares += 1;

        let sh = &mut ctx.accounts.shares;
        if sh.job == Pubkey::default() {
            sh.job = job_key;
            sh.miner = miner;
            sh.epoch = epoch.index;
            sh.bump = ctx.bumps.shares;
        }
        sh.count += 1;
        emit!(ShareAccepted { job: job_key, miner, nonce, epoch: epoch.index, count: sh.count });
        Ok(())
    }

    /// Permissionless: fix a job's subsidy S for a closed epoch.
    pub fn settle_job(ctx: Context<SettleJob>, epoch_index: u64) -> Result<()> {
        let _ = epoch_index; // used by the account seeds
        let now = Clock::get()?.slot;
        require!(now >= ctx.accounts.epoch.end_slot, MiningError::EpochNotOver);
        settle(
            &mut ctx.accounts.config,
            &mut ctx.accounts.epoch,
            &mut ctx.accounts.job_stats,
            &mut ctx.accounts.job_epoch,
        );
        Ok(())
    }

    /// Miner pulls its lamports for one job in a closed epoch. Settles the job first
    /// if nobody has. Closes the Shares account (rent back to the miner).
    pub fn claim(ctx: Context<Claim>, epoch_index: u64) -> Result<()> {
        let _ = epoch_index; // used by the account seeds
        let now = Clock::get()?.slot;
        require!(now >= ctx.accounts.epoch.end_slot, MiningError::EpochNotOver);
        settle(
            &mut ctx.accounts.config,
            &mut ctx.accounts.epoch,
            &mut ctx.accounts.job_stats,
            &mut ctx.accounts.job_epoch,
        );
        let je = &mut ctx.accounts.job_epoch;
        let count = ctx.accounts.shares.count;
        let amount = reward::miner_reward(je.subsidy, count, je.total_shares, je.claimed_shares, je.paid);
        je.claimed_shares += count;
        je.paid += amount;
        if amount > 0 {
            let pool = ctx.accounts.mining_pool.to_account_info();
            let miner = ctx.accounts.miner.to_account_info();
            let mut pl = pool.try_borrow_mut_lamports()?;
            let mut ml = miner.try_borrow_mut_lamports()?;
            **pl = pl.checked_sub(amount).ok_or(MiningError::PoolEmpty)?;
            **ml = ml.checked_add(amount).ok_or(MiningError::Overflow)?;
        }
        let c = &mut ctx.accounts.config;
        c.outstanding = c.outstanding.saturating_sub(amount);
        emit!(Claimed {
            epoch: je.epoch,
            job: je.job,
            miner: ctx.accounts.miner.key(),
            amount,
        });
        Ok(())
    }
}

pub fn share_message(job: &Pubkey, miner: &Pubkey, nonce: u64) -> Vec<u8> {
    let mut m = Vec::with_capacity(SHARE_DOMAIN.len() + 72);
    m.extend_from_slice(SHARE_DOMAIN);
    m.extend_from_slice(job.as_ref());
    m.extend_from_slice(miner.as_ref());
    m.extend_from_slice(&nonce.to_le_bytes());
    m
}

fn pool_free(pool: &AccountInfo, outstanding: u64) -> Result<u64> {
    let rent = Rent::get()?.minimum_balance(pool.data_len());
    Ok(pool.lamports().saturating_sub(rent).saturating_sub(outstanding))
}

fn settle(c: &mut MiningConfig, e: &mut Epoch, js: &mut JobStats, je: &mut JobEpoch) {
    if je.settled {
        return;
    }
    let cap_remaining = js.cap.saturating_sub(js.subsidy_used);
    let s = reward::job_subsidy(e.budget, je.fee_weight, e.total_fee, cap_remaining);
    js.subsidy_used += s;
    je.subsidy = s;
    je.settled = true;
    e.allocated += s;
    e.jobs_settled += 1;
    if e.jobs_settled == e.jobs_active && !e.released {
        c.outstanding = c.outstanding.saturating_sub(e.budget - e.allocated);
        e.released = true;
    }
}

fn apply_config(c: &mut MiningConfig, p: ConfigParams) -> Result<()> {
    if let Some(a) = p.admin {
        c.admin = a;
    }
    if let Some(a) = p.attesters {
        require!(a.len() <= MAX_ATTESTERS, MiningError::BadConfig);
        c.attesters = a;
    }
    if let Some(t) = p.threshold {
        c.threshold = t;
    }
    if let Some(s) = p.epoch_slots {
        c.epoch_slots = s;
    }
    if let Some(b) = p.emission_bps {
        c.emission_bps = b;
    }
    if let Some(a) = p.alpha_bps {
        c.alpha_bps = a;
    }
    if let Some(n) = p.max_nonce {
        c.max_nonce = n;
    }
    require!(c.threshold >= 1, MiningError::BadConfig);
    require!(c.attesters.is_empty() || c.threshold as usize <= c.attesters.len(), MiningError::BadConfig);
    require!(c.epoch_slots > 0, MiningError::BadConfig);
    require!(c.emission_bps as u64 <= reward::BPS, MiningError::BadConfig);
    require!((c.alpha_bps as u64) < reward::BPS, MiningError::BadConfig);
    require!(c.max_nonce >= 1 && c.max_nonce <= MAX_NONCE_CAP, MiningError::BadConfig);
    Ok(())
}

// ---------- accounts ----------

#[account]
#[derive(InitSpace)]
pub struct MiningConfig {
    pub admin: Pubkey,
    #[max_len(8)]
    pub attesters: Vec<Pubkey>,
    pub threshold: u8,
    pub epoch_slots: u64,
    pub emission_bps: u16,
    pub alpha_bps: u16,
    pub max_nonce: u32,
    pub current_epoch: u64,
    pub outstanding: u64,
    pub bump: u8,
}

#[account]
#[derive(InitSpace)]
pub struct MiningPool {
    pub bump: u8,
}

#[account]
#[derive(InitSpace)]
pub struct Epoch {
    pub index: u64,
    pub start_slot: u64,
    pub end_slot: u64,
    pub budget: u64,
    pub total_fee: u64,
    pub allocated: u64,
    pub jobs_active: u32,
    pub jobs_settled: u32,
    pub released: bool,
    pub bump: u8,
}

#[account]
#[derive(InitSpace)]
pub struct JobStats {
    pub job: Pubkey,
    pub protocol_fee: u64,
    pub cap: u64,
    pub subsidy_used: u64,
    pub bump: u8,
}

#[account]
#[derive(InitSpace)]
pub struct JobEpoch {
    pub job: Pubkey,
    pub epoch: u64,
    pub fee_weight: u64,
    pub total_shares: u64,
    pub settled: bool,
    pub subsidy: u64,
    pub claimed_shares: u64,
    pub paid: u64,
    pub bump: u8,
}

#[account]
#[derive(InitSpace)]
pub struct Shares {
    pub job: Pubkey,
    pub miner: Pubkey,
    pub epoch: u64,
    pub count: u64,
    pub bump: u8,
}

#[account]
pub struct NonceBitmap {
    pub job: Pubkey,
    pub miner: Pubkey,
    pub capacity: u32,
    pub used: u32,
    pub bump: u8,
    pub bits: Vec<u8>,
}

impl NonceBitmap {
    pub fn space(max_nonce: u32) -> usize {
        8 + 32 + 32 + 4 + 4 + 1 + 4 + max_nonce.min(MAX_NONCE_CAP).div_ceil(8) as usize
    }
}

#[derive(AnchorSerialize, AnchorDeserialize, Clone, Default)]
pub struct ConfigParams {
    pub admin: Option<Pubkey>,
    pub attesters: Option<Vec<Pubkey>>,
    pub threshold: Option<u8>,
    pub epoch_slots: Option<u64>,
    pub emission_bps: Option<u16>,
    pub alpha_bps: Option<u16>,
    pub max_nonce: Option<u32>,
}

#[derive(Accounts)]
pub struct Initialize<'info> {
    #[account(mut)]
    pub admin: Signer<'info>,
    #[account(init, payer = admin, space = 8 + MiningConfig::INIT_SPACE, seeds = [CONFIG_SEED], bump)]
    pub config: Box<Account<'info, MiningConfig>>,
    #[account(init, payer = admin, space = 8 + MiningPool::INIT_SPACE, seeds = [POOL_SEED], bump)]
    pub mining_pool: Account<'info, MiningPool>,
    #[account(init, payer = admin, space = 8 + Epoch::INIT_SPACE, seeds = [EPOCH_SEED, &0u64.to_le_bytes()], bump)]
    pub epoch: Box<Account<'info, Epoch>>,
    pub system_program: Program<'info, System>,
}

#[derive(Accounts)]
pub struct UpdateConfig<'info> {
    pub admin: Signer<'info>,
    #[account(mut, seeds = [CONFIG_SEED], bump = config.bump, has_one = admin)]
    pub config: Box<Account<'info, MiningConfig>>,
}

#[derive(Accounts)]
pub struct AdvanceEpoch<'info> {
    #[account(mut)]
    pub payer: Signer<'info>,
    #[account(mut, seeds = [CONFIG_SEED], bump = config.bump)]
    pub config: Box<Account<'info, MiningConfig>>,
    #[account(seeds = [POOL_SEED], bump = mining_pool.bump)]
    pub mining_pool: Account<'info, MiningPool>,
    #[account(mut, seeds = [EPOCH_SEED, &config.current_epoch.to_le_bytes()], bump = current_epoch.bump)]
    pub current_epoch: Box<Account<'info, Epoch>>,
    #[account(
        init,
        payer = payer,
        space = 8 + Epoch::INIT_SPACE,
        seeds = [EPOCH_SEED, &(config.current_epoch + 1).to_le_bytes()],
        bump
    )]
    pub next_epoch: Box<Account<'info, Epoch>>,
    pub system_program: Program<'info, System>,
}

#[derive(Accounts)]
pub struct SubmitShare<'info> {
    #[account(mut)]
    pub miner: Signer<'info>,
    #[account(seeds = [CONFIG_SEED], bump = config.bump)]
    pub config: Box<Account<'info, MiningConfig>>,
    #[account(mut, seeds = [EPOCH_SEED, &config.current_epoch.to_le_bytes()], bump = epoch.bump)]
    pub epoch: Box<Account<'info, Epoch>>,
    pub job: Box<Account<'info, Job>>,
    #[account(
        init_if_needed,
        payer = miner,
        space = 8 + JobStats::INIT_SPACE,
        seeds = [JOB_STATS_SEED, job.key().as_ref()],
        bump
    )]
    pub job_stats: Box<Account<'info, JobStats>>,
    #[account(
        init_if_needed,
        payer = miner,
        space = 8 + JobEpoch::INIT_SPACE,
        seeds = [JOB_EPOCH_SEED, &epoch.index.to_le_bytes(), job.key().as_ref()],
        bump
    )]
    pub job_epoch: Box<Account<'info, JobEpoch>>,
    #[account(
        init_if_needed,
        payer = miner,
        space = 8 + Shares::INIT_SPACE,
        seeds = [SHARES_SEED, &epoch.index.to_le_bytes(), job.key().as_ref(), miner.key().as_ref()],
        bump
    )]
    pub shares: Box<Account<'info, Shares>>,
    #[account(
        init_if_needed,
        payer = miner,
        space = NonceBitmap::space(config.max_nonce),
        seeds = [NONCES_SEED, job.key().as_ref(), miner.key().as_ref()],
        bump
    )]
    pub nonces: Box<Account<'info, NonceBitmap>>,
    /// CHECK: instructions sysvar
    #[account(address = anchor_lang::solana_program::sysvar::instructions::ID)]
    pub instructions: UncheckedAccount<'info>,
    pub system_program: Program<'info, System>,
}

#[derive(Accounts)]
#[instruction(epoch_index: u64)]
pub struct SettleJob<'info> {
    #[account(mut, seeds = [CONFIG_SEED], bump = config.bump)]
    pub config: Box<Account<'info, MiningConfig>>,
    #[account(mut, seeds = [EPOCH_SEED, &epoch_index.to_le_bytes()], bump = epoch.bump)]
    pub epoch: Box<Account<'info, Epoch>>,
    /// CHECK: only its key is used (the job account may already be closed)
    pub job: UncheckedAccount<'info>,
    #[account(mut, seeds = [JOB_STATS_SEED, job.key().as_ref()], bump = job_stats.bump)]
    pub job_stats: Box<Account<'info, JobStats>>,
    #[account(mut, seeds = [JOB_EPOCH_SEED, &epoch_index.to_le_bytes(), job.key().as_ref()], bump = job_epoch.bump)]
    pub job_epoch: Box<Account<'info, JobEpoch>>,
}

#[derive(Accounts)]
#[instruction(epoch_index: u64)]
pub struct Claim<'info> {
    #[account(mut)]
    pub miner: Signer<'info>,
    #[account(mut, seeds = [CONFIG_SEED], bump = config.bump)]
    pub config: Box<Account<'info, MiningConfig>>,
    #[account(mut, seeds = [POOL_SEED], bump = mining_pool.bump)]
    pub mining_pool: Account<'info, MiningPool>,
    #[account(mut, seeds = [EPOCH_SEED, &epoch_index.to_le_bytes()], bump = epoch.bump)]
    pub epoch: Box<Account<'info, Epoch>>,
    /// CHECK: only its key is used (the job account may already be closed)
    pub job: UncheckedAccount<'info>,
    #[account(mut, seeds = [JOB_STATS_SEED, job.key().as_ref()], bump = job_stats.bump)]
    pub job_stats: Box<Account<'info, JobStats>>,
    #[account(mut, seeds = [JOB_EPOCH_SEED, &epoch_index.to_le_bytes(), job.key().as_ref()], bump = job_epoch.bump)]
    pub job_epoch: Box<Account<'info, JobEpoch>>,
    #[account(
        mut,
        close = miner,
        seeds = [SHARES_SEED, &epoch_index.to_le_bytes(), job.key().as_ref(), miner.key().as_ref()],
        bump = shares.bump
    )]
    pub shares: Box<Account<'info, Shares>>,
}

// ---------- events and errors ----------

#[event]
pub struct EpochStarted {
    pub index: u64,
    pub start_slot: u64,
    pub budget: u64,
}

#[event]
pub struct ShareAccepted {
    pub job: Pubkey,
    pub miner: Pubkey,
    pub nonce: u64,
    pub epoch: u64,
    pub count: u64,
}

#[event]
pub struct Claimed {
    pub epoch: u64,
    pub job: Pubkey,
    pub miner: Pubkey,
    pub amount: u64,
}

#[error_code]
pub enum MiningError {
    #[msg("invalid config value")]
    BadConfig,
    #[msg("current epoch has not ended")]
    EpochNotOver,
    #[msg("current epoch is over; call advance_epoch first")]
    EpochOver,
    #[msg("job is not open for mining")]
    JobNotOpen,
    #[msg("nonce out of range")]
    NonceOutOfRange,
    #[msg("share already submitted for this (job, miner, nonce)")]
    ShareReplayed,
    #[msg("mining pool cannot pay")]
    PoolEmpty,
    #[msg("overflow")]
    Overflow,
}
