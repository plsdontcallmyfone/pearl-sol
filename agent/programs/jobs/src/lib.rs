//! AI job market on the agent chain (SPEC 3.3, ERC-8183 style).
//!
//! A job is a matrix multiply AB that an agent pays for. The poster escrows the
//! worker pay W in the job PDA and pays the protocol fee P into the mining program's
//! `mining_pool` PDA at post time (non-refundable).
//!
//! ## The 76-byte header and terms_hash
//!
//! `header = job_pubkey (32) || terms_hash (32) || created_slot u64 LE (8) || target_bits u32 LE (4)`
//! where `target_bits = share_target` and
//!
//! ```text
//! terms_hash = sha256(
//!     b"pearl-sol:terms:v1" || poster [32] || job_id u64 || m u32 || n u32 || k u32
//!     || worker_pay u64 || protocol_fee u64 || deadline_slot u64 || share_target u32 )
//! ```
//!
//! (all integers little-endian). Left out on purpose:
//! - `hash_a`, `hash_b`: they are BLAKE3 keyed by `job_key = blake3(header || config)`,
//!   so they cannot also be inside the header (circular).
//! - `config`: it is hashed into `job_key` directly.
//! - `data_uri`: only a pointer; the data itself is bound by `hash_a` / `hash_b`.
//!
//! `created_slot` is chosen by the poster, not read at execution, because the poster
//! must know the header before posting to compute `hash_a` / `hash_b`. The program
//! requires `clock.slot - config.max_slot_lag <= created_slot <= clock.slot`.
//! The job PDA is `["job", poster, job_id u64 LE]`, so its address is also known
//! in advance. Anyone can rebuild the header from the job account (it is also stored).

use anchor_lang::prelude::*;
use anchor_lang::solana_program::hash::hashv;
use anchor_lang::system_program;

pub mod sigcheck;

declare_id!("7e9pEhjwEqKemjC6zUugp4isi2zQMvaZPhn9kdnDuzqL");

pub const MINING_PROGRAM_ID: Pubkey = pubkey!("3GrrgQMSHC3PjkW6UmvQJPGK1WBvhxiz3tkigafj3GhP");
pub const MINING_POOL_SEED: &[u8] = b"mining_pool";
pub const JOB_SEED: &[u8] = b"job";
pub const JOBS_CONFIG_SEED: &[u8] = b"jobs_config";
pub const TERMS_DOMAIN: &[u8] = b"pearl-sol:terms:v1";
pub const RESULT_DOMAIN: &[u8] = b"pearl-sol:result:v1";
pub const CONFIG_LEN: usize = 52;
pub const HEADER_LEN: usize = 76;
pub const MAX_URI_LEN: usize = 200;
pub const MAX_EVALUATORS: usize = 8;

#[program]
pub mod jobs {
    use super::*;

    /// One-time setup. The signer becomes admin.
    pub fn initialize_config(ctx: Context<InitializeConfig>, params: ConfigParams) -> Result<()> {
        let c = &mut ctx.accounts.config;
        c.admin = ctx.accounts.admin.key();
        c.evaluators = vec![];
        c.threshold = 1;
        c.min_protocol_fee = 0;
        c.max_slot_lag = 150;
        c.max_deadline_slots = 9000 * 24 * 30;
        c.bump = ctx.bumps.config;
        apply_config(c, params)
    }

    /// Admin edits any field. `None` keeps the current value.
    pub fn update_config(ctx: Context<UpdateConfig>, params: ConfigParams) -> Result<()> {
        apply_config(&mut ctx.accounts.config, params)
    }

    pub fn post_job(ctx: Context<PostJob>, p: PostJobParams) -> Result<()> {
        let cfg = &ctx.accounts.config;
        let now = Clock::get()?.slot;
        require!(p.created_slot <= now, JobsError::BadCreatedSlot);
        require!(now - p.created_slot <= cfg.max_slot_lag, JobsError::BadCreatedSlot);
        require!(p.deadline_slot > now, JobsError::BadDeadline);
        require!(p.deadline_slot - now <= cfg.max_deadline_slots, JobsError::BadDeadline);
        require!(p.m > 0 && p.n > 0 && p.k > 0, JobsError::BadDims);
        require!(p.worker_pay > 0, JobsError::BadAmount);
        require!(p.protocol_fee >= cfg.min_protocol_fee, JobsError::FeeTooLow);
        require!(p.data_uri.len() <= MAX_URI_LEN, JobsError::UriTooLong);

        let poster = ctx.accounts.poster.key();
        let job_key = ctx.accounts.job.key();
        let terms_hash = terms_hash(&poster, &p);
        let header = build_header(&job_key, &terms_hash, p.created_slot, p.share_target);

        let j = &mut ctx.accounts.job;
        j.poster = poster;
        j.job_id = p.job_id;
        j.status = JobStatus::Open;
        j.hash_a = p.hash_a;
        j.hash_b = p.hash_b;
        j.config = p.config;
        j.m = p.m;
        j.n = p.n;
        j.k = p.k;
        j.data_uri = p.data_uri;
        j.worker_pay = p.worker_pay;
        j.protocol_fee = p.protocol_fee;
        j.created_slot = p.created_slot;
        j.posted_slot = now;
        j.deadline_slot = p.deadline_slot;
        j.share_target = p.share_target;
        j.terms_hash = terms_hash;
        j.header = header;
        j.deliverer = Pubkey::default();
        j.result_hash = [0; 32];
        j.result_uri = String::new();
        j.delivered_slot = 0;
        j.bump = ctx.bumps.job;

        let sys = ctx.accounts.system_program.to_account_info();
        system_program::transfer(
            CpiContext::new(
                sys.clone(),
                system_program::Transfer {
                    from: ctx.accounts.poster.to_account_info(),
                    to: ctx.accounts.job.to_account_info(),
                },
            ),
            p.worker_pay,
        )?;
        if p.protocol_fee > 0 {
            system_program::transfer(
                CpiContext::new(
                    sys,
                    system_program::Transfer {
                        from: ctx.accounts.poster.to_account_info(),
                        to: ctx.accounts.mining_pool.to_account_info(),
                    },
                ),
                p.protocol_fee,
            )?;
        }
        emit!(JobPosted {
            job: job_key,
            poster,
            worker_pay: p.worker_pay,
            protocol_fee: p.protocol_fee,
            header,
        });
        Ok(())
    }

    /// The miner who will deliver AB posts a commitment to it. First one wins.
    pub fn submit_result(ctx: Context<SubmitResult>, result_hash: [u8; 32], result_uri: String) -> Result<()> {
        require!(result_uri.len() <= MAX_URI_LEN, JobsError::UriTooLong);
        let now = Clock::get()?.slot;
        let j = &mut ctx.accounts.job;
        require!(j.status == JobStatus::Open, JobsError::BadStatus);
        require!(now <= j.deadline_slot, JobsError::PastDeadline);
        j.deliverer = ctx.accounts.deliverer.key();
        j.result_hash = result_hash;
        j.result_uri = result_uri;
        j.delivered_slot = now;
        j.status = JobStatus::Delivered;
        emit!(ResultSubmitted { job: j.key(), deliverer: j.deliverer, result_hash });
        Ok(())
    }

    /// Evaluator path: the attester set signed
    /// `b"pearl-sol:result:v1" || job || deliverer || result_hash` (ed25519 precompile
    /// instruction in the same transaction). Anyone may send it. Releases W.
    pub fn complete(ctx: Context<Complete>) -> Result<()> {
        let j = &ctx.accounts.job;
        require!(
            j.status == JobStatus::Delivered || j.status == JobStatus::Disputed,
            JobsError::BadStatus
        );
        let msg = result_message(&j.key(), &j.deliverer, &j.result_hash);
        let cfg = &ctx.accounts.config;
        sigcheck::require_signed(&ctx.accounts.instructions, &msg, &cfg.evaluators, cfg.threshold)?;
        pay_worker(&mut ctx.accounts.job, &ctx.accounts.deliverer)
    }

    /// Poster path: the poster accepts the delivered result directly. Releases W.
    pub fn accept(ctx: Context<Accept>) -> Result<()> {
        let j = &ctx.accounts.job;
        require!(
            j.status == JobStatus::Delivered || j.status == JobStatus::Disputed,
            JobsError::BadStatus
        );
        pay_worker(&mut ctx.accounts.job, &ctx.accounts.deliverer)
    }

    /// Poster rejects the delivered result. The evaluator can still complete it with
    /// a signed Result (it re-checked AB), or the admin resolves it.
    pub fn dispute(ctx: Context<Dispute>) -> Result<()> {
        let j = &mut ctx.accounts.job;
        require!(j.status == JobStatus::Delivered, JobsError::BadStatus);
        j.status = JobStatus::Disputed;
        emit!(JobDisputed { job: j.key() });
        Ok(())
    }

    /// Admin settles a dispute: pay the deliverer, or refund the poster.
    pub fn resolve(ctx: Context<Resolve>, to_deliverer: bool) -> Result<()> {
        require!(ctx.accounts.job.status == JobStatus::Disputed, JobsError::BadStatus);
        if to_deliverer {
            pay_worker(&mut ctx.accounts.job, &ctx.accounts.deliverer)
        } else {
            let w = ctx.accounts.job.worker_pay;
            move_lamports(&ctx.accounts.job.to_account_info(), &ctx.accounts.poster, w)?;
            ctx.accounts.job.status = JobStatus::Refunded;
            emit!(JobRefunded { job: ctx.accounts.job.key(), amount: w });
            Ok(())
        }
    }

    /// Permissionless: an Open job past its deadline returns W to the poster.
    pub fn expire(ctx: Context<Expire>) -> Result<()> {
        let now = Clock::get()?.slot;
        let j = &ctx.accounts.job;
        require!(j.status == JobStatus::Open, JobsError::BadStatus);
        require!(now > j.deadline_slot, JobsError::NotExpired);
        let w = j.worker_pay;
        move_lamports(&ctx.accounts.job.to_account_info(), &ctx.accounts.poster, w)?;
        ctx.accounts.job.status = JobStatus::Expired;
        emit!(JobExpired { job: ctx.accounts.job.key(), refunded: w });
        Ok(())
    }

    /// Poster reclaims the job account rent once the job is terminal.
    pub fn close_job(ctx: Context<CloseJob>) -> Result<()> {
        let s = ctx.accounts.job.status;
        require!(
            matches!(s, JobStatus::Completed | JobStatus::Expired | JobStatus::Refunded),
            JobsError::BadStatus
        );
        Ok(())
    }
}

// ---------- pure helpers (shared with the CLI and tests) ----------

pub fn terms_hash(poster: &Pubkey, p: &PostJobParams) -> [u8; 32] {
    hashv(&[
        TERMS_DOMAIN,
        poster.as_ref(),
        &p.job_id.to_le_bytes(),
        &p.m.to_le_bytes(),
        &p.n.to_le_bytes(),
        &p.k.to_le_bytes(),
        &p.worker_pay.to_le_bytes(),
        &p.protocol_fee.to_le_bytes(),
        &p.deadline_slot.to_le_bytes(),
        &p.share_target.to_le_bytes(),
    ])
    .to_bytes()
}

pub fn build_header(job: &Pubkey, terms_hash: &[u8; 32], created_slot: u64, target_bits: u32) -> [u8; HEADER_LEN] {
    let mut h = [0u8; HEADER_LEN];
    h[0..32].copy_from_slice(job.as_ref());
    h[32..64].copy_from_slice(terms_hash);
    h[64..72].copy_from_slice(&created_slot.to_le_bytes());
    h[72..76].copy_from_slice(&target_bits.to_le_bytes());
    h
}

pub fn result_message(job: &Pubkey, deliverer: &Pubkey, result_hash: &[u8; 32]) -> Vec<u8> {
    let mut m = Vec::with_capacity(RESULT_DOMAIN.len() + 96);
    m.extend_from_slice(RESULT_DOMAIN);
    m.extend_from_slice(job.as_ref());
    m.extend_from_slice(deliverer.as_ref());
    m.extend_from_slice(result_hash);
    m
}

fn move_lamports(from: &AccountInfo, to: &AccountInfo, amount: u64) -> Result<()> {
    let mut f = from.try_borrow_mut_lamports()?;
    let mut t = to.try_borrow_mut_lamports()?;
    **f = f.checked_sub(amount).ok_or(JobsError::Insufficient)?;
    **t = t.checked_add(amount).ok_or(JobsError::Overflow)?;
    Ok(())
}

fn pay_worker<'info>(job: &mut Account<'info, Job>, deliverer: &AccountInfo<'info>) -> Result<()> {
    let w = job.worker_pay;
    move_lamports(&job.to_account_info(), deliverer, w)?;
    job.status = JobStatus::Completed;
    emit!(JobCompleted { job: job.key(), deliverer: job.deliverer, amount: w });
    Ok(())
}

fn apply_config(c: &mut JobsConfig, p: ConfigParams) -> Result<()> {
    if let Some(a) = p.admin {
        c.admin = a;
    }
    if let Some(e) = p.evaluators {
        require!(e.len() <= MAX_EVALUATORS, JobsError::TooManyEvaluators);
        c.evaluators = e;
    }
    if let Some(t) = p.threshold {
        c.threshold = t;
    }
    if let Some(f) = p.min_protocol_fee {
        c.min_protocol_fee = f;
    }
    if let Some(l) = p.max_slot_lag {
        c.max_slot_lag = l;
    }
    if let Some(d) = p.max_deadline_slots {
        c.max_deadline_slots = d;
    }
    require!(c.threshold >= 1, JobsError::BadThreshold);
    require!(
        c.evaluators.is_empty() || c.threshold as usize <= c.evaluators.len(),
        JobsError::BadThreshold
    );
    Ok(())
}

// ---------- accounts ----------

#[derive(AnchorSerialize, AnchorDeserialize, Clone, Copy, PartialEq, Eq, Debug, InitSpace)]
pub enum JobStatus {
    Open,
    Delivered,
    Completed,
    Disputed,
    Expired,
    Refunded,
}

#[account]
#[derive(InitSpace)]
pub struct JobsConfig {
    pub admin: Pubkey,
    #[max_len(8)]
    pub evaluators: Vec<Pubkey>,
    pub threshold: u8,
    pub min_protocol_fee: u64,
    pub max_slot_lag: u64,
    pub max_deadline_slots: u64,
    pub bump: u8,
}

#[account]
#[derive(InitSpace)]
pub struct Job {
    pub poster: Pubkey,
    pub job_id: u64,
    pub status: JobStatus,
    pub hash_a: [u8; 32],
    pub hash_b: [u8; 32],
    pub config: [u8; 52],
    pub m: u32,
    pub n: u32,
    pub k: u32,
    #[max_len(200)]
    pub data_uri: String,
    pub worker_pay: u64,
    pub protocol_fee: u64,
    pub created_slot: u64,
    pub posted_slot: u64,
    pub deadline_slot: u64,
    pub share_target: u32,
    pub terms_hash: [u8; 32],
    pub header: [u8; 76],
    pub deliverer: Pubkey,
    pub result_hash: [u8; 32],
    #[max_len(200)]
    pub result_uri: String,
    pub delivered_slot: u64,
    pub bump: u8,
}

#[derive(AnchorSerialize, AnchorDeserialize, Clone, Default)]
pub struct ConfigParams {
    pub admin: Option<Pubkey>,
    pub evaluators: Option<Vec<Pubkey>>,
    pub threshold: Option<u8>,
    pub min_protocol_fee: Option<u64>,
    pub max_slot_lag: Option<u64>,
    pub max_deadline_slots: Option<u64>,
}

#[derive(AnchorSerialize, AnchorDeserialize, Clone)]
pub struct PostJobParams {
    pub job_id: u64,
    pub hash_a: [u8; 32],
    pub hash_b: [u8; 32],
    pub config: [u8; 52],
    pub m: u32,
    pub n: u32,
    pub k: u32,
    pub data_uri: String,
    pub worker_pay: u64,
    pub protocol_fee: u64,
    pub created_slot: u64,
    pub deadline_slot: u64,
    pub share_target: u32,
}

#[derive(Accounts)]
pub struct InitializeConfig<'info> {
    #[account(mut)]
    pub admin: Signer<'info>,
    #[account(init, payer = admin, space = 8 + JobsConfig::INIT_SPACE, seeds = [JOBS_CONFIG_SEED], bump)]
    pub config: Account<'info, JobsConfig>,
    pub system_program: Program<'info, System>,
}

#[derive(Accounts)]
pub struct UpdateConfig<'info> {
    pub admin: Signer<'info>,
    #[account(mut, seeds = [JOBS_CONFIG_SEED], bump = config.bump, has_one = admin)]
    pub config: Account<'info, JobsConfig>,
}

#[derive(Accounts)]
#[instruction(p: PostJobParams)]
pub struct PostJob<'info> {
    #[account(mut)]
    pub poster: Signer<'info>,
    #[account(seeds = [JOBS_CONFIG_SEED], bump = config.bump)]
    pub config: Account<'info, JobsConfig>,
    #[account(
        init,
        payer = poster,
        space = 8 + Job::INIT_SPACE,
        seeds = [JOB_SEED, poster.key().as_ref(), &p.job_id.to_le_bytes()],
        bump
    )]
    pub job: Box<Account<'info, Job>>,
    /// CHECK: the mining program's pool PDA; address checked by seeds.
    #[account(mut, seeds = [MINING_POOL_SEED], bump, seeds::program = MINING_PROGRAM_ID)]
    pub mining_pool: UncheckedAccount<'info>,
    pub system_program: Program<'info, System>,
}

#[derive(Accounts)]
pub struct SubmitResult<'info> {
    pub deliverer: Signer<'info>,
    #[account(mut)]
    pub job: Box<Account<'info, Job>>,
}

#[derive(Accounts)]
pub struct Complete<'info> {
    #[account(seeds = [JOBS_CONFIG_SEED], bump = config.bump)]
    pub config: Account<'info, JobsConfig>,
    #[account(mut, has_one = deliverer)]
    pub job: Box<Account<'info, Job>>,
    /// CHECK: must equal job.deliverer (has_one); receives W.
    #[account(mut)]
    pub deliverer: UncheckedAccount<'info>,
    /// CHECK: instructions sysvar, address checked in sigcheck.
    #[account(address = anchor_lang::solana_program::sysvar::instructions::ID)]
    pub instructions: UncheckedAccount<'info>,
}

#[derive(Accounts)]
pub struct Accept<'info> {
    pub poster: Signer<'info>,
    #[account(mut, has_one = poster, has_one = deliverer)]
    pub job: Box<Account<'info, Job>>,
    /// CHECK: must equal job.deliverer (has_one); receives W.
    #[account(mut)]
    pub deliverer: UncheckedAccount<'info>,
}

#[derive(Accounts)]
pub struct Dispute<'info> {
    pub poster: Signer<'info>,
    #[account(mut, has_one = poster)]
    pub job: Box<Account<'info, Job>>,
}

#[derive(Accounts)]
pub struct Resolve<'info> {
    pub admin: Signer<'info>,
    #[account(seeds = [JOBS_CONFIG_SEED], bump = config.bump, has_one = admin)]
    pub config: Account<'info, JobsConfig>,
    #[account(mut, has_one = poster, has_one = deliverer)]
    pub job: Box<Account<'info, Job>>,
    /// CHECK: job.poster (has_one)
    #[account(mut)]
    pub poster: UncheckedAccount<'info>,
    /// CHECK: job.deliverer (has_one)
    #[account(mut)]
    pub deliverer: UncheckedAccount<'info>,
}

#[derive(Accounts)]
pub struct Expire<'info> {
    #[account(mut, has_one = poster)]
    pub job: Box<Account<'info, Job>>,
    /// CHECK: job.poster (has_one); receives the refund.
    #[account(mut)]
    pub poster: UncheckedAccount<'info>,
}

#[derive(Accounts)]
pub struct CloseJob<'info> {
    #[account(mut)]
    pub poster: Signer<'info>,
    #[account(mut, close = poster, has_one = poster)]
    pub job: Box<Account<'info, Job>>,
}

// ---------- events and errors ----------

#[event]
pub struct JobPosted {
    pub job: Pubkey,
    pub poster: Pubkey,
    pub worker_pay: u64,
    pub protocol_fee: u64,
    pub header: [u8; 76],
}

#[event]
pub struct ResultSubmitted {
    pub job: Pubkey,
    pub deliverer: Pubkey,
    pub result_hash: [u8; 32],
}

#[event]
pub struct JobCompleted {
    pub job: Pubkey,
    pub deliverer: Pubkey,
    pub amount: u64,
}

#[event]
pub struct JobDisputed {
    pub job: Pubkey,
}

#[event]
pub struct JobRefunded {
    pub job: Pubkey,
    pub amount: u64,
}

#[event]
pub struct JobExpired {
    pub job: Pubkey,
    pub refunded: u64,
}

#[error_code]
pub enum JobsError {
    #[msg("created_slot must be a recent slot, not in the future")]
    BadCreatedSlot,
    #[msg("deadline out of range")]
    BadDeadline,
    #[msg("m, n, k must be non-zero")]
    BadDims,
    #[msg("amount must be non-zero")]
    BadAmount,
    #[msg("protocol fee below config minimum")]
    FeeTooLow,
    #[msg("uri longer than 200 bytes")]
    UriTooLong,
    #[msg("job is not in the required status")]
    BadStatus,
    #[msg("job deadline has passed")]
    PastDeadline,
    #[msg("job has not reached its deadline")]
    NotExpired,
    #[msg("insufficient lamports")]
    Insufficient,
    #[msg("overflow")]
    Overflow,
    #[msg("too many evaluators")]
    TooManyEvaluators,
    #[msg("bad threshold")]
    BadThreshold,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Cross-language vector: services/agent-cli `selftest` checks the same bytes.
    #[test]
    fn terms_hash_vector() {
        let p = PostJobParams {
            job_id: 7,
            hash_a: [0; 32],
            hash_b: [0; 32],
            config: [0; 52],
            m: 64,
            n: 128,
            k: 1024,
            data_uri: String::new(),
            worker_pay: 2_000_000_000,
            protocol_fee: 500_000_000,
            created_slot: 99,
            deadline_slot: 123_456,
            share_target: 0x207f_ffff,
        };
        let th = terms_hash(&Pubkey::new_from_array([1; 32]), &p);
        let hex: String = th.iter().map(|b| format!("{b:02x}")).collect();
        assert_eq!(hex, "a7769ee3567d6dff4da7a3a846682838cd07084c3d506c7568331d798b7e0031");
        let h = build_header(&Pubkey::new_from_array([2; 32]), &th, 99, 0x207f_ffff);
        assert_eq!(&h[0..32], &[2; 32]);
        assert_eq!(&h[64..72], &99u64.to_le_bytes());
        assert_eq!(&h[72..76], &[0xff, 0xff, 0x7f, 0x20]);
    }

    #[test]
    fn result_message_layout() {
        let m = result_message(&Pubkey::new_from_array([1; 32]), &Pubkey::new_from_array([2; 32]), &[3; 32]);
        assert_eq!(m.len(), 19 + 96);
        assert_eq!(&m[..19], b"pearl-sol:result:v1");
    }
}
