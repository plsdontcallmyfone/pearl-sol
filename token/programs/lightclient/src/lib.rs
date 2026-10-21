//! pearl-sol light client (SPEC v2 5.3): follows the agent chain's PoW headers on
//! Solana and verifies the work, so the bridge and the storefront can act on proofs
//! against final headers instead of an M-of-N signer set.
//!
//! Flow per header (all permissionless; a relayer pays rent and fees, gets the rent
//! back when the header is closed and an optional reward from the pool):
//! 1. `submit_header(hash, header)`: parent must be a submitted header (any status, so
//!    a relay can submit and verify many headers in parallel). Checks version,
//!    height = parent + 1, prev_hash, `target_bits` = WTEMA(parent) (exact port of
//!    chain/consensus-pow), timestamp > median of the last 11, timestamp not beyond the
//!    Solana clock + `max_future_ms`, target within the pow limit. Stores the header
//!    with cumulative work, status Pending.
//! 2. Proof upload into a relayer-owned work account: `begin_verify` (binds the work
//!    account to the header and the job), `write_proof` chunks.
//! 3. Verify steps (any number per transaction, in this order):
//!    `verify_proof_hash` (blake3(proof) = header.proof_hash in 1 KiB-chunk ranges, then
//!    PSJ1 parse, dims, tile, leaf sets; freezes the buffer), fallback blocks only: `fallback_gen_b`,
//!    `fallback_hash_bt`, `fallback_hash_a` (rebuild hash_b / hash_a from pow_digest in
//!    ranges), then `verify_opening(A)`, `verify_opening(B^T)`, `verify_jackpot`
//!    preceded by `verify_noise(A)`, `verify_noise(B^T)` (seeds, noise; then jackpot,
//!    rank-penalized target; noise and jackpot run in ranges). The last step marks the
//!    header Verified when its parent is Verified (the parent is the tip, or is passed
//!    as the one remaining account) and, when it extends the tip, moves the tip;
//!    otherwise WorkDone (its own work checked, an ancestor still unverified).
//!    `connect(header)` later promotes a WorkDone header whose parent became Verified.
//!    Invariant: Verified = own work verified AND parent Verified, so by induction from
//!    the checkpoint every Verified header has a fully verified ancestry, exactly as
//!    when headers had to be verified one at a time (SPEC 5.3 "parallel relay").
//! 4. `reorg(new_tip)`: switch to a heavier verified branch (path headers passed as
//!    remaining accounts), refused if the fork point is below the final floor
//!    (tip - K, never decreasing).
//! 5. `mark_final(header)`: sets `is_final` on a canonical header at or below the final
//!    floor. Bridge and storefront proofs need `is_final`.
//! Job blocks need the job's commitments: `register_job` proves the PostJob (or the
//! storefront-order BridgeUnlock) tx against the `tx_root` of a verified canonical
//! header and stores hash_a, hash_b, m, n, config and the Pearl job header.
//!
//! Final-header accumulator (Agent Credit R5; `Accumulator ["acc"]`): a SHA-256 Merkle
//! mountain range (crates/pearl-verify-sbf `acc`) over the hashes of FINAL headers in
//! height order, from the final header at `init_accumulator` on. `accumulate` appends
//! the next final headers (permissionless; each must link by prev_hash to the last one);
//! `close_header` refuses a final header not yet accumulated, so every final header
//! from the start stays provable forever: `prove_header` checks a header's MMR
//! inclusion against the current root or one of the last `ACC_ROOTS` roots (and returns
//! its hash and height as return data), `restore_header` re-creates a closed final
//! `HeaderRecord` from such a proof, so every existing reader (`read_final_header`)
//! works on it again. Other programs can check inclusion inline with
//! [`verify_accumulated`] over the raw `Accumulator` account.

use anchor_lang::prelude::*;
use anchor_lang::solana_program::hash::hashv;
use pearl_verify_sbf as pv;
use pv::difficulty::{self, Retarget};
use pv::header::Header;
use pv::verify::{JobParams, Side};

// Localnet id = keys/localnet/lightclient-keypair.json; the public devnet build (`--features devnet`,
// docs/DEVNET.md) uses its own program keypair (keys/devnet/, not committed).
#[cfg(not(feature = "devnet"))]
declare_id!("BUDUW2bGexRGRHhEkBUj8dAvffRuXQQacredxKtRMHK5");
#[cfg(feature = "devnet")]
declare_id!("47mavKipT2URcdj9qp2Stv6H5cM41cmzDXiTEcmDie4m");

pub const CONFIG_SEED: &[u8] = b"config";
pub const CANON_SEED: &[u8] = b"canon";
pub const HEADER_SEED: &[u8] = b"header";
pub const JOB_SEED: &[u8] = b"job";
pub const POOL_SEED: &[u8] = b"pool";
pub const ACC_SEED: &[u8] = b"acc";

/// Recent accumulator roots kept (by leaf count mod ACC_ROOTS), so a proof built against a
/// slightly older size still verifies after a few appends.
pub const ACC_ROOTS: usize = 128;

/// Canonical-chain ring (by height mod CANON_LEN).
pub const CANON_LEN: usize = 256;
pub const MTP_SPAN: usize = 11;

pub const STATUS_PENDING: u8 = 1;
pub const STATUS_VERIFIED: u8 = 2;
/// Own work verified, waiting for its parent to be Verified (`connect`).
pub const STATUS_WORK_DONE: u8 = 3;

pub const SIDE_A: u8 = 0;
pub const SIDE_BT: u8 = 1;

pub fn sha256(parts: &[&[u8]]) -> [u8; 32] {
    hashv(parts).to_bytes()
}

pub fn header_pda(hash: &[u8; 32]) -> (Pubkey, u8) {
    Pubkey::find_program_address(&[HEADER_SEED, hash], &ID)
}

pub fn job_pda(job_id: &[u8; 32]) -> (Pubkey, u8) {
    Pubkey::find_program_address(&[JOB_SEED, job_id], &ID)
}

pub fn acc_pda() -> (Pubkey, u8) {
    Pubkey::find_program_address(&[ACC_SEED], &ID)
}

fn sha_parts(parts: &[&[u8]]) -> [u8; 32] {
    hashv(parts).to_bytes()
}

pub mod work;
use work::Work;

#[cfg(not(target_os = "solana"))]
pub mod client;

#[derive(AnchorSerialize, AnchorDeserialize, Clone, Copy, Debug, PartialEq, Eq, InitSpace)]
pub struct Params {
    pub chain_id: u32,
    /// Finality depth K (chain parameter finality_k).
    pub finality_k: u32,
    pub target_block_time_ms: u64,
    pub half_life_ms: u64,
    pub pow_limit_bits: u32,
    pub min_difficulty_bits: u32,
    pub max_future_ms: u64,
    pub fallback_m: u32,
    pub fallback_n: u32,
    pub fallback_config: [u8; 52],
    /// Lamports paid from the pool to the relayer of each verified header.
    pub relay_reward: u64,
    /// Final headers this many blocks below the final floor may be closed.
    pub retain_final: u64,
}

impl Params {
    fn check(&self) -> Result<()> {
        require!(self.finality_k >= 1 && (self.finality_k as usize) < CANON_LEN - 16, LcError::BadParams);
        require!(self.half_life_ms > 0 && self.target_block_time_ms > 0, LcError::BadParams);
        require!(!difficulty::nbits_to_target(self.pow_limit_bits).is_zero(), LcError::BadParams);
        let cfg = pv::config::MiningConfig::from_bytes(&self.fallback_config).map_err(|_| error!(LcError::BadParams))?;
        cfg.check_shape(self.fallback_m, self.fallback_n).map_err(|_| error!(LcError::BadParams))?;
        Ok(())
    }
}

#[program]
pub mod lightclient {
    use super::*;

    /// Trusted start: a checkpoint header (genesis or a recent final header) with its
    /// cumulative work and up to 11 timestamps (its own first, then its ancestors').
    pub fn initialize(
        ctx: Context<Initialize>,
        checkpoint_hash: [u8; 32],
        params: Params,
        checkpoint: [u8; 256],
        cum_work: [u8; 32],
        ts_hist: Vec<u64>,
    ) -> Result<()> {
        params.check()?;
        let h = Header(&checkpoint);
        require!(h.block_hash() == checkpoint_hash, LcError::HashMismatch);
        require!(!ts_hist.is_empty() && ts_hist.len() <= MTP_SPAN && ts_hist[0] == h.timestamp_ms(), LcError::BadParams);
        require!(h.height() == 0 || ts_hist.len() >= 2, LcError::BadParams);
        let c = &mut ctx.accounts.config;
        c.admin = ctx.accounts.admin.key();
        c.params = params;
        c.checkpoint_hash = checkpoint_hash;
        c.checkpoint_height = h.height();
        c.tip_hash = checkpoint_hash;
        c.tip_height = h.height();
        c.tip_work = cum_work;
        c.final_floor = h.height();
        c.final_hash = checkpoint_hash;
        c.final_height = h.height();
        c.bump = ctx.bumps.config;
        c.pool_bump = ctx.bumps.pool;
        let mut canon = ctx.accounts.canon.load_init()?;
        canon.hashes[(h.height() as usize) % CANON_LEN] = checkpoint_hash;
        let mut r = ctx.accounts.header.load_init()?;
        r.hash = checkpoint_hash;
        r.header = checkpoint;
        r.cum_work = cum_work;
        r.height = h.height();
        for (i, t) in ts_hist.iter().enumerate() {
            r.ts_hist[i] = *t;
        }
        r.ts_len = ts_hist.len() as u8;
        r.status = STATUS_VERIFIED;
        r.is_final = 1;
        r.bump = ctx.bumps.header;
        r.relayer = ctx.accounts.admin.key();
        r.verified_slot = Clock::get()?.slot;
        emit!(TipChanged { hash: checkpoint_hash, height: h.height(), reorg_depth: 0 });
        Ok(())
    }

    /// Every parameter is admin-editable (mirror AdminConfig changes made on the chain).
    pub fn set_params(ctx: Context<AdminOnly>, params: Params) -> Result<()> {
        params.check()?;
        ctx.accounts.config.params = params;
        Ok(())
    }

    pub fn set_admin(ctx: Context<AdminOnly>, new_admin: Pubkey) -> Result<()> {
        ctx.accounts.config.admin = new_admin;
        Ok(())
    }

    /// Store a header whose parent is verified, after the header-only checks.
    pub fn submit_header(ctx: Context<SubmitHeader>, hash: [u8; 32], header: [u8; 256]) -> Result<()> {
        let h = Header(&header);
        require!(h.block_hash() == hash, LcError::HashMismatch);
        require!(h.version() == pv::header::VERSION, LcError::BadHeader);
        let p = &ctx.accounts.config.params;
        let parent = ctx.accounts.parent.load()?;
        // Any submitted parent: its fields were checked against its own parent when it
        // was submitted. Nothing here trusts the parent's work; a header only becomes
        // Verified once its whole ancestry is (see verify_jackpot / connect).
        require!(matches!(parent.status, STATUS_PENDING | STATUS_VERIFIED | STATUS_WORK_DONE), LcError::ParentNotVerified);
        require!(h.prev_hash() == parent.hash, LcError::BadHeader);
        require!(h.height() == parent.height + 1, LcError::BadHeader);
        // Below the final floor nothing can change.
        require!(h.height() > ctx.accounts.config.final_floor, LcError::TooDeep);
        // Difficulty: Pearl WTEMA on the parent's own solve time.
        let pts = parent.ts_hist[0];
        let gp = if parent.height == 0 { None } else { Some(parent.ts_hist[1]) };
        let want = difficulty::next_bits(&Retarget {
            parent_bits: Header(&parent.header).target_bits(),
            parent_ts_ms: pts,
            grandparent_ts_ms: gp,
            target_block_time_ms: p.target_block_time_ms,
            half_life_ms: p.half_life_ms,
            pow_limit_bits: p.pow_limit_bits,
            min_difficulty_bits: p.min_difficulty_bits,
        });
        if h.target_bits() != want {
            msg!("target_bits {:#x}, required {:#x}", h.target_bits(), want);
            return err!(LcError::WrongTarget);
        }
        let t = difficulty::nbits_to_target(h.target_bits());
        require!(!t.is_zero() && t <= difficulty::nbits_to_target(p.pow_limit_bits), LcError::WrongTarget);
        // Timestamps: after the median of the last 11, not too far ahead of Solana time.
        let n = parent.ts_len as usize;
        let mut ts = [0u64; MTP_SPAN];
        ts[..n].copy_from_slice(&parent.ts_hist[..n]);
        ts[..n].sort_unstable();
        let mtp = ts[n / 2];
        require!(h.timestamp_ms() > mtp, LcError::Timestamp);
        let now_ms = (Clock::get()?.unix_timestamp.max(0) as u64).saturating_mul(1000);
        require!(h.timestamp_ms() <= now_ms.saturating_add(p.max_future_ms), LcError::Timestamp);
        // Work.
        let cw = difficulty::u256_from_le(&parent.cum_work).saturating_add(difficulty::work(h.target_bits()));
        let mut r = ctx.accounts.header.load_init()?;
        r.hash = hash;
        r.header = header;
        r.cum_work = difficulty::u256_le(&cw);
        r.height = h.height();
        r.ts_hist[0] = h.timestamp_ms();
        let m = n.min(MTP_SPAN - 1);
        r.ts_hist[1..1 + m].copy_from_slice(&parent.ts_hist[..m]);
        r.ts_len = (m + 1) as u8;
        r.status = STATUS_PENDING;
        r.bump = ctx.bumps.header;
        r.relayer = ctx.accounts.payer.key();
        emit!(HeaderSubmitted { hash, height: h.height() });
        Ok(())
    }

    /// Set up a relayer-owned work account (created beforehand with any size, owner =
    /// this program): `proof_cap` bytes for proofs, the rest is fallback scratch.
    pub fn init_work(ctx: Context<InitWork>, proof_cap: u32) -> Result<()> {
        let info = ctx.accounts.work.to_account_info();
        Work::init(&info, &ctx.accounts.authority.key(), proof_cap)
    }

    /// Bind the work account to a pending header (resets all progress). Job blocks pass
    /// the registered `JobCommit` as the only remaining account.
    pub fn begin_verify<'info>(ctx: Context<'_, '_, 'info, 'info, BeginVerify<'info>>, proof_len: u32) -> Result<()> {
        let rec = ctx.accounts.header.load()?;
        require!(rec.status == STATUS_PENDING, LcError::NotPending);
        let h = Header(&rec.header);
        let info = ctx.accounts.work.to_account_info();
        let mut w = Work::load(&info, &ctx.accounts.authority.key())?;
        let p = &ctx.accounts.config.params;
        let (params, cfg52) = if h.is_fallback() {
            let pd = h.pow_digest();
            let cfg = pv::config::MiningConfig::from_bytes(&p.fallback_config).map_err(|_| error!(LcError::BadParams))?;
            (
                JobParams {
                    job_key: pv::verify::job_key(&pv::fallback::job_header(&pd), &p.fallback_config),
                    config: cfg,
                    m: p.fallback_m,
                    n: p.fallback_n,
                    hash_a: [0; 32],
                    hash_b: [0; 32],
                },
                p.fallback_config,
            )
        } else {
            let acc = ctx.remaining_accounts.first().ok_or(error!(LcError::JobMissing))?;
            require_keys_eq!(*acc.owner, crate::ID, LcError::JobMissing);
            let job: Account<JobCommit> = Account::try_from(acc)?;
            require!(job.job_id == h.job_id(), LcError::JobMissing);
            require!(job.posted_height < h.height(), LcError::JobMissing);
            (
                JobParams {
                    job_key: job.job_key,
                    config: pv::config::MiningConfig::from_bytes(&job.config).map_err(|_| error!(LcError::JobMissing))?,
                    m: job.m,
                    n: job.n,
                    hash_a: job.hash_a,
                    hash_b: job.hash_b,
                },
                job.config,
            )
        };
        w.begin(&rec.hash, proof_len, h.is_fallback(), &params, &cfg52)?;
        Ok(())
    }

    pub fn write_proof(ctx: Context<WorkOnly>, offset: u32, data: Vec<u8>) -> Result<()> {
        let info = ctx.accounts.work.to_account_info();
        let mut w = Work::load(&info, &ctx.accounts.authority.key())?;
        w.write_proof(offset, &data)
    }

    /// Step 1: proof_hash, parse, dims, tile, exact leaf sets. Freezes the proof bytes.
    /// Resumable: `count` 1 KiB chunks of the proof hash per call.
    pub fn verify_proof_hash(ctx: Context<VerifyStep>, count: u32) -> Result<()> {
        let rec = ctx.accounts.header.load()?;
        let info = ctx.accounts.work.to_account_info();
        let mut w = Work::load(&info, &ctx.accounts.authority.key())?;
        w.check_target(&rec)?;
        w.proof_hash_step(&Header(&rec.header), count as u64)
    }

    /// Fallback blocks: generate `count` 64-byte XOF blocks of B into scratch.
    pub fn fallback_gen_b(ctx: Context<VerifyStep>, count: u32) -> Result<()> {
        let rec = ctx.accounts.header.load()?;
        let info = ctx.accounts.work.to_account_info();
        let mut w = Work::load(&info, &ctx.accounts.authority.key())?;
        w.check_target(&rec)?;
        w.gen_b_step(&Header(&rec.header).pow_digest(), count as u64)
    }

    /// Fallback blocks: hash `count` chunks of B^T (after B is generated).
    pub fn fallback_hash_bt(ctx: Context<VerifyStep>, count: u32) -> Result<()> {
        let rec = ctx.accounts.header.load()?;
        let info = ctx.accounts.work.to_account_info();
        let mut w = Work::load(&info, &ctx.accounts.authority.key())?;
        w.check_target(&rec)?;
        w.hash_bt_step(count as u64)
    }

    /// Fallback blocks: hash `count` chunks of A (straight from the XOF).
    pub fn fallback_hash_a(ctx: Context<VerifyStep>, count: u32) -> Result<()> {
        let rec = ctx.accounts.header.load()?;
        let info = ctx.accounts.work.to_account_info();
        let mut w = Work::load(&info, &ctx.accounts.authority.key())?;
        w.check_target(&rec)?;
        w.hash_a_step(&Header(&rec.header).pow_digest(), count as u64)
    }

    /// Step 2: one opening (side 0 = A, 1 = B^T) against its commitment.
    pub fn verify_opening(ctx: Context<VerifyStep>, side: u8) -> Result<()> {
        let rec = ctx.accounts.header.load()?;
        let info = ctx.accounts.work.to_account_info();
        let mut w = Work::load(&info, &ctx.accounts.authority.key())?;
        w.check_target(&rec)?;
        let s = match side {
            SIDE_A => Side::A,
            SIDE_BT => Side::Bt,
            _ => return err!(LcError::BadStep),
        };
        w.opening_step(s)
    }

    /// Step 3a: one side's noised strips (side 0 = A, 1 = B^T) into the work account,
    /// `count` 8-column noise blocks per call (resumable).
    pub fn verify_noise(ctx: Context<VerifyStep>, side: u8, count: u32) -> Result<()> {
        let rec = ctx.accounts.header.load()?;
        let info = ctx.accounts.work.to_account_info();
        let mut w = Work::load(&info, &ctx.accounts.authority.key())?;
        w.check_target(&rec)?;
        let s = match side {
            SIDE_A => Side::A,
            SIDE_BT => Side::Bt,
            _ => return err!(LcError::BadStep),
        };
        w.noise_step(s, &Header(&rec.header).pow_digest(), count)
    }

    /// Step 3b: jackpot, `count` rank blocks per call (resumable). The call that completes
    /// it checks the target. Then, if the parent is Verified (it is the tip, or it is the
    /// optional remaining account), the header is marked Verified, extends the tip when
    /// the parent is the tip, and the relay reward is paid; otherwise it is marked
    /// WorkDone and waits for `connect`.
    pub fn verify_jackpot<'info>(ctx: Context<'_, '_, 'info, 'info, VerifyFinal<'info>>, count: u32) -> Result<()> {
        let prev = {
            let rec = ctx.accounts.header.load()?;
            let info = ctx.accounts.work.to_account_info();
            let mut w = Work::load(&info, &ctx.accounts.authority.key())?;
            w.check_target(&rec)?;
            let h = Header(&rec.header);
            if !w.jackpot_step(&h.pow_digest(), h.target_bits(), count)? {
                return Ok(());
            }
            h.prev_hash()
        };
        let parent_verified = prev == ctx.accounts.config.tip_hash
            || match ctx.remaining_accounts.first() {
                Some(ai) if *ai.owner == crate::ID => {
                    let loader = AccountLoader::<HeaderRecord>::try_from(ai)?;
                    let p = loader.load()?;
                    p.hash == prev && p.status == STATUS_VERIFIED
                }
                _ => false,
            };
        if !parent_verified {
            let mut rec = ctx.accounts.header.load_mut()?;
            rec.status = STATUS_WORK_DONE;
            emit!(WorkVerified { hash: rec.hash, height: rec.height });
            return Ok(());
        }
        let a = ctx.accounts;
        promote(&a.header, &mut a.config, &a.canon, &a.pool.to_account_info(), &a.relayer.to_account_info())
    }

    /// Promote a WorkDone header whose parent is now Verified: marks it Verified, extends
    /// the tip when the parent is the tip, pays the relay reward. Permissionless; several
    /// in one tx (parent first) connect a run of headers.
    pub fn connect(ctx: Context<Connect>) -> Result<()> {
        {
            let p = ctx.accounts.parent.load()?;
            let r = ctx.accounts.header.load()?;
            require!(r.status == STATUS_WORK_DONE, LcError::NotWorkDone);
            require!(p.status == STATUS_VERIFIED && Header(&r.header).prev_hash() == p.hash, LcError::ParentNotVerified);
        }
        let a = ctx.accounts;
        promote(&a.header, &mut a.config, &a.canon, &a.pool.to_account_info(), &a.relayer.to_account_info())
    }

    /// Close a relayer-owned work account, lamports back to its authority.
    pub fn close_work(ctx: Context<CloseWork>) -> Result<()> {
        let info = ctx.accounts.work.to_account_info();
        {
            let _w = Work::load(&info, &ctx.accounts.authority.key())?;
        }
        let auth = ctx.accounts.authority.to_account_info();
        let l = info.lamports();
        **info.try_borrow_mut_lamports()? = 0;
        **auth.try_borrow_mut_lamports()? += l;
        info.try_borrow_mut_data()?.fill(0);
        Ok(())
    }

    /// Switch the tip to a heavier verified branch. Remaining accounts: the new tip's
    /// ancestors, newest first, ending with the first one already canonical (the fork
    /// point). Refused below the final floor.
    pub fn reorg<'info>(ctx: Context<'_, '_, 'info, 'info, Reorg<'info>>) -> Result<()> {
        let (new_hash, new_height, new_work, mut prev) = {
            let t = ctx.accounts.new_tip.load()?;
            require!(t.status == STATUS_VERIFIED, LcError::NotVerified);
            (t.hash, t.height, t.cum_work, Header(&t.header).prev_hash())
        };
        let c = &mut ctx.accounts.config;
        require!(difficulty::u256_from_le(&new_work) > difficulty::u256_from_le(&c.tip_work), LcError::NotHeavier);
        let mut canon = ctx.accounts.canon.load_mut()?;
        let mut branch: Vec<([u8; 32], u64)> = vec![(new_hash, new_height)];
        let mut fork_height = None;
        for acc in ctx.remaining_accounts.iter() {
            require_keys_eq!(*acc.owner, crate::ID, LcError::BadPath);
            let loader = AccountLoader::<HeaderRecord>::try_from(acc)?;
            let r = loader.load()?;
            require!(r.hash == prev && r.status == STATUS_VERIFIED, LcError::BadPath);
            if is_canonical(&canon, c, &r.hash, r.height) {
                fork_height = Some(r.height);
                break;
            }
            branch.push((r.hash, r.height));
            prev = Header(&r.header).prev_hash();
        }
        let f = fork_height.ok_or(error!(LcError::BadPath))?;
        require!(f >= c.final_floor, LcError::TooDeep);
        require!(c.tip_height.saturating_sub(f) as usize <= CANON_LEN - 1 && (new_height - f) as usize <= CANON_LEN - 1, LcError::TooDeep);
        for (hsh, ht) in &branch {
            canon.hashes[(*ht as usize) % CANON_LEN] = *hsh;
        }
        let depth = c.tip_height - f;
        c.tip_hash = new_hash;
        c.tip_height = new_height;
        c.tip_work = new_work;
        c.final_floor = c.final_floor.max(new_height.saturating_sub(c.params.finality_k as u64));
        emit!(TipChanged { hash: new_hash, height: new_height, reorg_depth: depth });
        Ok(())
    }

    /// Mark a canonical header at or below the final floor as final (permanent). A header
    /// that fell out of the canonical ring (more than CANON_LEN below the tip) is final
    /// when its child is: pass that final child as the one remaining account
    /// (security pass 2026-10-21, finding L-2: otherwise such headers could never be
    /// marked or accumulated and the accumulator would stall for good).
    pub fn mark_final<'info>(ctx: Context<'_, '_, 'info, 'info, MarkFinal<'info>>) -> Result<()> {
        let c = &mut ctx.accounts.config;
        let canon = ctx.accounts.canon.load()?;
        let mut r = ctx.accounts.header.load_mut()?;
        require!(r.status == STATUS_VERIFIED, LcError::NotVerified);
        require!(r.height <= c.final_floor, LcError::NotFinal);
        let canonical = is_canonical(&canon, c, &r.hash, r.height) || has_final_child(ctx.remaining_accounts, &r.hash, r.height)?;
        require!(canonical, LcError::NotCanonical);
        r.is_final = 1;
        if r.height > c.final_height {
            c.final_height = r.height;
            c.final_hash = r.hash;
        }
        emit!(HeaderFinal { hash: r.hash, height: r.height });
        Ok(())
    }

    /// Record a job's commitments from the tx that created it (PostJob, a storefront-order
    /// BridgeUnlock, or a PostSegments for a standing job), proved against a verified
    /// canonical header.
    pub fn register_job(ctx: Context<RegisterJob>, job_id: [u8; 32], tx: Vec<u8>, path: Vec<u8>) -> Result<()> {
        let c = &ctx.accounts.config;
        let canon = ctx.accounts.canon.load()?;
        let block = ctx.accounts.block.load()?;
        require!(block.status == STATUS_VERIFIED, LcError::NotVerified);
        require!(block.is_final == 1 || is_canonical(&canon, c, &block.hash, block.height), LcError::NotCanonical);
        let h = Header(&block.header);
        let th = pv::chaintx::tx_hash(&tx);
        pv::chaintx::verify_tx_path(&th, &path, &h.tx_root()).map_err(|e| verr(e))?;
        let view = pv::chaintx::TxView::new(&tx, c.params.chain_id).map_err(|e| verr(e))?;
        // PostJob, order BridgeUnlock, or one standing job of a PostSegments (SPEC 4.5)
        let jc = pv::chaintx::job_by_id(&view, &job_id, sha256).map_err(|e| verr(e))?;
        require!(jc.job_id == job_id, LcError::JobMissing);
        let cfg = pv::config::MiningConfig::from_bytes(&jc.config).map_err(|e| verr(e))?;
        cfg.check_shape(jc.m, jc.n).map_err(|e| verr(e))?;
        let j = &mut ctx.accounts.job;
        j.job_id = jc.job_id;
        j.job_key = pv::verify::job_key(&jc.header76, &jc.config);
        j.header76 = jc.header76;
        j.config = jc.config;
        j.m = jc.m;
        j.n = jc.n;
        j.hash_a = jc.hash_a;
        j.hash_b = jc.hash_b;
        j.posted_height = block.height;
        j.posted_block = block.hash;
        j.bump = ctx.bumps.job;
        emit!(JobRegistered { job_id, posted_height: block.height });
        Ok(())
    }

    /// Re-register a job whose `JobCommit` came from a block that is no longer canonical
    /// (security pass 2026-10-21, finding L-1). The light client checks only proof of work,
    /// not transactions, so one PoW-valid block carrying a fake PostJob / order BridgeUnlock
    /// could register `job_id` first with junk commitments, and the real job's blocks would
    /// then never verify (the light client halts). `witness` is the final or canonical
    /// header at the old commit's `posted_height` and differs from its `posted_block`;
    /// `block` (Verified, final or canonical) carries the tx, proved as in `register_job`.
    pub fn replace_job(ctx: Context<ReplaceJob>, job_id: [u8; 32], tx: Vec<u8>, path: Vec<u8>) -> Result<()> {
        let c = &ctx.accounts.config;
        let canon = ctx.accounts.canon.load()?;
        {
            let w = ctx.accounts.witness.load()?;
            let j = &ctx.accounts.job;
            require!(proves_dead(&canon, c, &w, &j.posted_block, j.posted_height), LcError::NotCanonical);
        }
        let block = ctx.accounts.block.load()?;
        require!(block.status == STATUS_VERIFIED, LcError::NotVerified);
        require!(block.is_final == 1 || is_canonical(&canon, c, &block.hash, block.height), LcError::NotCanonical);
        let h = Header(&block.header);
        let th = pv::chaintx::tx_hash(&tx);
        pv::chaintx::verify_tx_path(&th, &path, &h.tx_root()).map_err(|e| verr(e))?;
        let view = pv::chaintx::TxView::new(&tx, c.params.chain_id).map_err(|e| verr(e))?;
        let jc = pv::chaintx::job_by_id(&view, &job_id, sha256).map_err(|e| verr(e))?;
        require!(jc.job_id == job_id, LcError::JobMissing);
        let cfg = pv::config::MiningConfig::from_bytes(&jc.config).map_err(|e| verr(e))?;
        cfg.check_shape(jc.m, jc.n).map_err(|e| verr(e))?;
        let j = &mut ctx.accounts.job;
        j.job_key = pv::verify::job_key(&jc.header76, &jc.config);
        j.header76 = jc.header76;
        j.config = jc.config;
        j.m = jc.m;
        j.n = jc.n;
        j.hash_a = jc.hash_a;
        j.hash_b = jc.hash_b;
        j.posted_height = block.height;
        j.posted_block = block.hash;
        emit!(JobRegistered { job_id, posted_height: block.height });
        Ok(())
    }

    /// Admin: start the final-header accumulator at the current final header (leaf 0).
    /// Needed once (Initialize does not create it, so older deployments can add it).
    pub fn init_accumulator(ctx: Context<InitAccumulator>) -> Result<()> {
        let c = &ctx.accounts.config;
        let mut a = ctx.accounts.acc.load_init()?;
        a.start_height = c.final_height;
        a.last_hash = c.final_hash;
        a.count = 0;
        pv::acc::append(sha_parts, 0, &mut a.peaks, &c.final_hash);
        a.count = 1;
        let root = pv::acc::root_of_slots(sha_parts, 1, &a.peaks);
        a.roots[1 % ACC_ROOTS] = root;
        a.bump = ctx.bumps.acc;
        emit!(Accumulated { height: c.final_height, hash: c.final_hash, count: 1, root });
        Ok(())
    }

    /// Append the next final headers (remaining accounts, `HeaderRecord`s in height order).
    /// Permissionless. Each must be verified, final (or canonical at or below the final
    /// floor), at height `start_height + count` and the child of the last appended header.
    pub fn accumulate<'info>(ctx: Context<'_, '_, 'info, 'info, Accumulate<'info>>) -> Result<()> {
        let c = &ctx.accounts.config;
        let canon = ctx.accounts.canon.load()?;
        let mut a = ctx.accounts.acc.load_mut()?;
        require!(!ctx.remaining_accounts.is_empty(), LcError::BadPath);
        for ai in ctx.remaining_accounts.iter() {
            require_keys_eq!(*ai.owner, crate::ID, LcError::BadPath);
            let loader = AccountLoader::<HeaderRecord>::try_from(ai)?;
            let r = loader.load()?;
            require!(r.status == STATUS_VERIFIED, LcError::NotVerified);
            require!(r.height == a.start_height + a.count, LcError::AccOrder);
            require!(Header(&r.header).prev_hash() == a.last_hash, LcError::AccOrder);
            let fin = r.is_final == 1 || (r.height <= c.final_floor && is_canonical(&canon, c, &r.hash, r.height));
            require!(fin, LcError::NotFinal);
            let n = a.count;
            pv::acc::append(sha_parts, n, &mut a.peaks, &r.hash);
            a.count = n + 1;
            a.last_hash = r.hash;
            let root = pv::acc::root_of_slots(sha_parts, n + 1, &a.peaks);
            a.roots[((n + 1) as usize) % ACC_ROOTS] = root;
            emit!(Accumulated { height: r.height, hash: r.hash, count: n + 1, root });
        }
        Ok(())
    }

    /// Check that `header` is a final header in the accumulator: MMR inclusion of its hash
    /// at leaf `height - start_height`. `count` = the current leaf count with the short
    /// proof (siblings to its peak, checked against the stored peak; fits a tx at any
    /// size), or any count among the last ACC_ROOTS with the full proof (siblings and the
    /// other peaks, checked against that root; fits a tx up to about 2^15 leaves). Return data:
    /// `block_hash [32] || height u64 LE`. Read-only; callable by CPI.
    pub fn prove_header(ctx: Context<ProveHeader>, header: [u8; 256], count: u64, proof: Vec<u8>) -> Result<()> {
        let a = ctx.accounts.acc.load()?;
        let (hash, height) = a.verify(&header, count, &proof)?;
        let mut out = [0u8; 40];
        out[..32].copy_from_slice(&hash);
        out[32..].copy_from_slice(&height.to_le_bytes());
        anchor_lang::solana_program::program::set_return_data(&out);
        Ok(())
    }

    /// Re-create the `HeaderRecord` of a closed final header from an accumulator proof
    /// (as `prove_header`). The payer pays its rent and may close it again later. The
    /// record is Verified and final, `cum_work` zero, `_pad[0] = 1` (restored).
    pub fn restore_header(ctx: Context<RestoreHeader>, hash: [u8; 32], header: [u8; 256], count: u64, proof: Vec<u8>) -> Result<()> {
        let a = ctx.accounts.acc.load()?;
        let (h, height) = a.verify(&header, count, &proof)?;
        require!(h == hash, LcError::HashMismatch);
        let mut r = ctx.accounts.header.load_init()?;
        r.hash = hash;
        r.header = header;
        r.height = height;
        r.ts_hist[0] = Header(&header).timestamp_ms();
        r.ts_len = 1;
        r.status = STATUS_VERIFIED;
        r.is_final = 1;
        r.bump = ctx.bumps.header;
        r.relayer = ctx.accounts.payer.key();
        r.verified_slot = Clock::get()?.slot;
        r._pad[0] = 1;
        emit!(HeaderRestored { hash, height });
        Ok(())
    }

    /// Close a header no longer needed, rent back to its relayer: a final header
    /// `retain_final` below the final floor, a dead fork header at or below the floor,
    /// or a pending header whose height is at or below the floor.
    pub fn close_header(ctx: Context<CloseHeader>) -> Result<()> {
        let c = &ctx.accounts.config;
        let canon = ctx.accounts.canon.load()?;
        let r = ctx.accounts.header.load()?;
        require_keys_eq!(ctx.accounts.relayer.key(), r.relayer, LcError::WrongRelayer);
        require!(r.hash != c.tip_hash && r.hash != c.final_hash, LcError::CannotClose);
        let ok = if r.is_final == 1 {
            let old_enough = r.height + c.params.retain_final < c.final_floor;
            if old_enough {
                // A final header must be in the accumulator first (R5); headers below its
                // start height predate it.
                let acc = ctx.accounts.acc.load()?;
                require!(r.height < acc.start_height + acc.count, LcError::NotAccumulated);
            }
            old_enough
        } else {
            // A Verified header that left the canonical ring may still be canonical (not
            // marked final yet): keep it until the accumulator has passed its height, so a
            // final-gap can always be closed by `mark_final` with a final child (L-2).
            let in_ring = r.height <= c.tip_height && c.tip_height - r.height < CANON_LEN as u64;
            let past_acc = {
                let acc = ctx.accounts.acc.load()?;
                r.height < acc.start_height + acc.count
            };
            r.height <= c.final_floor
                && !(r.status == STATUS_VERIFIED && is_canonical(&canon, c, &r.hash, r.height))
                && (r.status != STATUS_VERIFIED || in_ring || past_acc)
        };
        require!(ok, LcError::CannotClose);
        Ok(())
    }
}

fn verr(e: pv::Error) -> Error {
    msg!("verifier: {:?}", e);
    error!(LcError::VerifyFailed)
}

fn is_canonical(canon: &Canon, c: &Config, hash: &[u8; 32], height: u64) -> bool {
    height <= c.tip_height && (c.tip_height - height) < CANON_LEN as u64 && canon.hashes[(height as usize) % CANON_LEN] == *hash
}

/// True when the first remaining account is a final, Verified `HeaderRecord` at
/// `height + 1` whose parent is `hash`: a final header's parent is canonical forever.
fn has_final_child<'info>(rem: &'info [AccountInfo<'info>], hash: &[u8; 32], height: u64) -> Result<bool> {
    let Some(ai) = rem.first() else { return Ok(false) };
    require_keys_eq!(*ai.owner, crate::ID, LcError::BadPath);
    let loader = AccountLoader::<HeaderRecord>::try_from(ai)?;
    let ch = loader.load()?;
    Ok(ch.is_final == 1 && ch.status == STATUS_VERIFIED && ch.height == height + 1 && Header(&ch.header).prev_hash() == *hash)
}

/// The HeaderRecord passed as `witness` proves that `old_block` (at `old_height`) is not
/// canonical: the witness is the final or canonical header at that height and differs.
fn proves_dead(canon: &Canon, c: &Config, w: &HeaderRecord, old_block: &[u8; 32], old_height: u64) -> bool {
    w.status == STATUS_VERIFIED
        && w.height == old_height
        && w.hash != *old_block
        && (w.is_final == 1 || is_canonical(canon, c, &w.hash, w.height))
}

/// Mark a header whose work is verified and whose parent is Verified as Verified; extend
/// the tip when the parent is the tip; pay the relay reward (best effort, never below
/// the pool's rent floor). Callers check the parent.
fn promote<'info>(
    header: &AccountLoader<'info, HeaderRecord>,
    c: &mut Account<'info, Config>,
    canon: &AccountLoader<'info, Canon>,
    pool: &AccountInfo<'info>,
    rcpt: &AccountInfo<'info>,
) -> Result<()> {
    let slot = Clock::get()?.slot;
    let (hash, height, prev, cw, relayer) = {
        let mut rec = header.load_mut()?;
        rec.status = STATUS_VERIFIED;
        rec.verified_slot = slot;
        (rec.hash, rec.height, Header(&rec.header).prev_hash(), rec.cum_work, rec.relayer)
    };
    emit!(HeaderVerified { hash, height });
    if prev == c.tip_hash && difficulty::u256_from_le(&cw) > difficulty::u256_from_le(&c.tip_work) {
        let mut canon = canon.load_mut()?;
        canon.hashes[(height as usize) % CANON_LEN] = hash;
        c.tip_hash = hash;
        c.tip_height = height;
        c.tip_work = cw;
        c.final_floor = c.final_floor.max(height.saturating_sub(c.params.finality_k as u64));
        emit!(TipChanged { hash, height, reorg_depth: 0 });
    }
    let reward = c.params.relay_reward;
    if reward > 0 {
        require_keys_eq!(rcpt.key(), relayer, LcError::WrongRelayer);
        let floor = Rent::get()?.minimum_balance(0);
        let pay = reward.min(pool.lamports().saturating_sub(floor));
        if pay > 0 {
            **pool.try_borrow_mut_lamports()? -= pay;
            **rcpt.try_borrow_mut_lamports()? += pay;
        }
    }
    Ok(())
}

// ---------------------------------------------------------------- accounts

#[account]
#[derive(InitSpace)]
pub struct Config {
    pub admin: Pubkey,
    pub params: Params,
    pub checkpoint_hash: [u8; 32],
    pub checkpoint_height: u64,
    pub tip_hash: [u8; 32],
    pub tip_height: u64,
    /// Cumulative work of the tip, U256 little-endian.
    pub tip_work: [u8; 32],
    /// Highest height that can never be reorged: max over time of tip - K.
    pub final_floor: u64,
    /// Highest header marked final.
    pub final_height: u64,
    pub final_hash: [u8; 32],
    pub bump: u8,
    pub pool_bump: u8,
}

#[account(zero_copy)]
#[repr(C)]
pub struct Canon {
    pub hashes: [[u8; 32]; CANON_LEN],
}

/// One header. Fixed layout (see [`layout`]) so other programs read it raw.
#[account(zero_copy)]
#[repr(C)]
pub struct HeaderRecord {
    pub hash: [u8; 32],
    pub header: [u8; 256],
    /// U256 little-endian.
    pub cum_work: [u8; 32],
    pub height: u64,
    /// Own timestamp first, then up to 10 ancestors'.
    pub ts_hist: [u64; MTP_SPAN],
    pub relayer: Pubkey,
    pub verified_slot: u64,
    pub ts_len: u8,
    pub status: u8,
    pub is_final: u8,
    pub bump: u8,
    pub _pad: [u8; 4],
}

/// Final-header accumulator (R5). Fixed layout (see [`acc_layout`]) so other programs
/// read it raw.
#[account(zero_copy)]
#[repr(C)]
pub struct Accumulator {
    /// Height of leaf 0.
    pub start_height: u64,
    /// Leaves (final headers) appended: heights start_height .. start_height + count.
    pub count: u64,
    /// Hash of the last appended header (height start_height + count - 1).
    pub last_hash: [u8; 32],
    /// MMR peaks by height: `peaks[b]` is the peak over 2^b leaves when bit b of count is set.
    pub peaks: [[u8; 32]; 64],
    /// `roots[n % ACC_ROOTS]` = root when the accumulator held n leaves.
    pub roots: [[u8; 32]; ACC_ROOTS],
    pub bump: u8,
    pub _pad: [u8; 7],
}

impl Accumulator {
    /// `(block_hash, height)` of `header` if it is in the accumulator at size `count`.
    pub fn verify(&self, header: &[u8; 256], count: u64, proof: &[u8]) -> Result<([u8; 32], u64)> {
        require!(count >= 1 && count <= self.count && self.count - count < ACC_ROOTS as u64, LcError::AccRoot);
        let h = Header(header);
        let height = h.height();
        require!(height >= self.start_height, LcError::AccProof);
        let i = height - self.start_height;
        let hash = h.block_hash();
        // Short form (siblings only) against the current peaks; else the full form against
        // the root at `count` (current or recent).
        if count == self.count && pv::acc::short_proof_len(count, i) == Some(proof.len()) {
            let (b, peak) = pv::acc::verify_to_peak(sha_parts, count, i, &hash, proof).map_err(|_| error!(LcError::AccProof))?;
            require!(self.peaks[b as usize] == peak, LcError::AccProof);
            return Ok((hash, height));
        }
        let root = self.roots[(count as usize) % ACC_ROOTS];
        pv::acc::verify(sha_parts, count, i, &hash, proof, &root).map_err(|_| error!(LcError::AccProof))?;
        Ok((hash, height))
    }
}

/// Byte offsets inside the `Accumulator` account (with the 8-byte discriminator).
pub mod acc_layout {
    pub const START_HEIGHT: usize = 8;
    pub const COUNT: usize = 16;
    pub const LAST_HASH: usize = 24;
    pub const PEAKS: usize = 56;
    pub const ROOTS: usize = PEAKS + 64 * 32;
    pub const LEN: usize = ROOTS + super::ACC_ROOTS * 32 + 8;
}

/// Raw-account reader for other programs (the caller checks the owner is this program
/// and the address is [`acc_pda`]): `(block_hash, height)` of `header` if it is a final
/// header in the accumulator at size `count`, using `sha256` (the Solana syscall).
pub fn verify_accumulated(acc_data: &[u8], header: &[u8; 256], count: u64, proof: &[u8]) -> Option<([u8; 32], u64)> {
    use acc_layout::*;
    use anchor_lang::Discriminator;
    if acc_data.len() < LEN || acc_data[..8] != *Accumulator::DISCRIMINATOR {
        return None;
    }
    let rd = |o: usize| u64::from_le_bytes(acc_data[o..o + 8].try_into().unwrap());
    let (start, have) = (rd(START_HEIGHT), rd(COUNT));
    if count == 0 || count > have || have - count >= ACC_ROOTS as u64 {
        return None;
    }
    let h = Header(header);
    let height = h.height();
    if height < start {
        return None;
    }
    let hash = h.block_hash();
    let i = height - start;
    if count == have && pv::acc::short_proof_len(count, i) == Some(proof.len()) {
        let (b, peak) = pv::acc::verify_to_peak(sha_parts, count, i, &hash, proof).ok()?;
        let po = PEAKS + 32 * b as usize;
        return (acc_data[po..po + 32] == peak).then_some((hash, height));
    }
    let ro = ROOTS + 32 * ((count as usize) % ACC_ROOTS);
    let root: [u8; 32] = acc_data[ro..ro + 32].try_into().unwrap();
    pv::acc::verify(sha_parts, count, i, &hash, proof, &root).ok()?;
    Some((hash, height))
}

/// Byte offsets inside a `HeaderRecord` account (with the 8-byte discriminator).
pub mod layout {
    pub const HASH: usize = 8;
    pub const HEADER: usize = 40;
    pub const TX_ROOT: usize = HEADER + 120;
    pub const STATE_ROOT: usize = HEADER + 152;
    pub const WITHDRAW_ROOT: usize = HEADER + 184;
    pub const CUM_WORK: usize = 296;
    pub const HEIGHT: usize = 328;
    pub const STATUS: usize = 8 + 457;
    pub const IS_FINAL: usize = 8 + 458;
    pub const LEN: usize = 8 + 464;
}

/// Read `(hash, header bytes)` of a FINAL header from raw account data; `None` if
/// the discriminator, size or final flag is wrong. Callers must check the owner.
pub fn read_final_header(data: &[u8]) -> Option<([u8; 32], [u8; 256])> {
    use anchor_lang::Discriminator;
    if data.len() < layout::LEN || data[..8] != *HeaderRecord::DISCRIMINATOR {
        return None;
    }
    if data[layout::IS_FINAL] != 1 || data[layout::STATUS] != STATUS_VERIFIED {
        return None;
    }
    Some((data[layout::HASH..layout::HASH + 32].try_into().unwrap(), data[layout::HEADER..layout::HEADER + 256].try_into().unwrap()))
}

#[account]
#[derive(InitSpace)]
pub struct JobCommit {
    pub job_id: [u8; 32],
    pub job_key: [u8; 32],
    pub header76: [u8; 76],
    pub config: [u8; 52],
    pub m: u32,
    pub n: u32,
    pub hash_a: [u8; 32],
    pub hash_b: [u8; 32],
    pub posted_height: u64,
    pub posted_block: [u8; 32],
    pub bump: u8,
}

#[derive(Accounts)]
#[instruction(checkpoint_hash: [u8; 32])]
pub struct Initialize<'info> {
    #[account(mut)]
    pub admin: Signer<'info>,
    #[account(init, payer = admin, space = 8 + Config::INIT_SPACE, seeds = [CONFIG_SEED], bump)]
    pub config: Box<Account<'info, Config>>,
    #[account(init, payer = admin, space = 8 + core::mem::size_of::<Canon>(), seeds = [CANON_SEED], bump)]
    pub canon: AccountLoader<'info, Canon>,
    #[account(init, payer = admin, space = layout::LEN, seeds = [HEADER_SEED, &checkpoint_hash], bump)]
    pub header: AccountLoader<'info, HeaderRecord>,
    /// CHECK: relay reward pool, a data-less PDA holding lamports for relayers.
    #[account(init, payer = admin, space = 0, seeds = [POOL_SEED], bump, owner = crate::ID)]
    pub pool: UncheckedAccount<'info>,
    pub system_program: Program<'info, System>,
}

#[derive(Accounts)]
pub struct AdminOnly<'info> {
    pub admin: Signer<'info>,
    #[account(mut, seeds = [CONFIG_SEED], bump = config.bump, has_one = admin @ LcError::NotAdmin)]
    pub config: Box<Account<'info, Config>>,
}

#[derive(Accounts)]
#[instruction(hash: [u8; 32])]
pub struct SubmitHeader<'info> {
    #[account(mut)]
    pub payer: Signer<'info>,
    #[account(seeds = [CONFIG_SEED], bump = config.bump)]
    pub config: Box<Account<'info, Config>>,
    pub parent: AccountLoader<'info, HeaderRecord>,
    #[account(init, payer = payer, space = layout::LEN, seeds = [HEADER_SEED, &hash], bump)]
    pub header: AccountLoader<'info, HeaderRecord>,
    pub system_program: Program<'info, System>,
}

#[derive(Accounts)]
pub struct InitWork<'info> {
    pub authority: Signer<'info>,
    /// CHECK: owner and layout checked in `Work::init`.
    #[account(mut, owner = crate::ID)]
    pub work: UncheckedAccount<'info>,
}

#[derive(Accounts)]
pub struct WorkOnly<'info> {
    pub authority: Signer<'info>,
    /// CHECK: owner and layout checked in `Work::load`.
    #[account(mut, owner = crate::ID)]
    pub work: UncheckedAccount<'info>,
}

#[derive(Accounts)]
pub struct BeginVerify<'info> {
    pub authority: Signer<'info>,
    /// CHECK: owner and layout checked in `Work::load`.
    #[account(mut, owner = crate::ID)]
    pub work: UncheckedAccount<'info>,
    #[account(seeds = [CONFIG_SEED], bump = config.bump)]
    pub config: Box<Account<'info, Config>>,
    pub header: AccountLoader<'info, HeaderRecord>,
}

#[derive(Accounts)]
pub struct VerifyStep<'info> {
    pub authority: Signer<'info>,
    /// CHECK: owner and layout checked in `Work::load`.
    #[account(mut, owner = crate::ID)]
    pub work: UncheckedAccount<'info>,
    pub header: AccountLoader<'info, HeaderRecord>,
}

#[derive(Accounts)]
pub struct VerifyFinal<'info> {
    pub authority: Signer<'info>,
    /// CHECK: owner and layout checked in `Work::load`.
    #[account(mut, owner = crate::ID)]
    pub work: UncheckedAccount<'info>,
    #[account(mut)]
    pub header: AccountLoader<'info, HeaderRecord>,
    #[account(mut, seeds = [CONFIG_SEED], bump = config.bump)]
    pub config: Box<Account<'info, Config>>,
    #[account(mut, seeds = [CANON_SEED], bump)]
    pub canon: AccountLoader<'info, Canon>,
    /// CHECK: the reward pool PDA.
    #[account(mut, seeds = [POOL_SEED], bump = config.pool_bump)]
    pub pool: UncheckedAccount<'info>,
    /// CHECK: must be the header's relayer (checked when a reward is paid).
    #[account(mut)]
    pub relayer: UncheckedAccount<'info>,
}

#[derive(Accounts)]
pub struct Connect<'info> {
    #[account(mut, seeds = [CONFIG_SEED], bump = config.bump)]
    pub config: Box<Account<'info, Config>>,
    #[account(mut, seeds = [CANON_SEED], bump)]
    pub canon: AccountLoader<'info, Canon>,
    /// CHECK: the reward pool PDA.
    #[account(mut, seeds = [POOL_SEED], bump = config.pool_bump)]
    pub pool: UncheckedAccount<'info>,
    pub parent: AccountLoader<'info, HeaderRecord>,
    #[account(mut)]
    pub header: AccountLoader<'info, HeaderRecord>,
    /// CHECK: must be the header's relayer (checked when a reward is paid).
    #[account(mut)]
    pub relayer: UncheckedAccount<'info>,
}

#[derive(Accounts)]
pub struct CloseWork<'info> {
    #[account(mut)]
    pub authority: Signer<'info>,
    /// CHECK: owner and layout checked in `Work::load`.
    #[account(mut, owner = crate::ID)]
    pub work: UncheckedAccount<'info>,
}

#[derive(Accounts)]
pub struct Reorg<'info> {
    #[account(mut, seeds = [CONFIG_SEED], bump = config.bump)]
    pub config: Box<Account<'info, Config>>,
    #[account(mut, seeds = [CANON_SEED], bump)]
    pub canon: AccountLoader<'info, Canon>,
    pub new_tip: AccountLoader<'info, HeaderRecord>,
}

#[derive(Accounts)]
pub struct MarkFinal<'info> {
    #[account(mut, seeds = [CONFIG_SEED], bump = config.bump)]
    pub config: Box<Account<'info, Config>>,
    #[account(seeds = [CANON_SEED], bump)]
    pub canon: AccountLoader<'info, Canon>,
    #[account(mut)]
    pub header: AccountLoader<'info, HeaderRecord>,
}

#[derive(Accounts)]
#[instruction(job_id: [u8; 32])]
pub struct RegisterJob<'info> {
    #[account(mut)]
    pub payer: Signer<'info>,
    #[account(seeds = [CONFIG_SEED], bump = config.bump)]
    pub config: Box<Account<'info, Config>>,
    #[account(seeds = [CANON_SEED], bump)]
    pub canon: AccountLoader<'info, Canon>,
    pub block: AccountLoader<'info, HeaderRecord>,
    #[account(init, payer = payer, space = 8 + JobCommit::INIT_SPACE, seeds = [JOB_SEED, &job_id], bump)]
    pub job: Box<Account<'info, JobCommit>>,
    pub system_program: Program<'info, System>,
}

#[derive(Accounts)]
#[instruction(job_id: [u8; 32])]
pub struct ReplaceJob<'info> {
    pub payer: Signer<'info>,
    #[account(seeds = [CONFIG_SEED], bump = config.bump)]
    pub config: Box<Account<'info, Config>>,
    #[account(seeds = [CANON_SEED], bump)]
    pub canon: AccountLoader<'info, Canon>,
    pub block: AccountLoader<'info, HeaderRecord>,
    /// The final or canonical header at the old commit's posted height.
    pub witness: AccountLoader<'info, HeaderRecord>,
    #[account(mut, seeds = [JOB_SEED, &job_id], bump = job.bump)]
    pub job: Box<Account<'info, JobCommit>>,
}

#[derive(Accounts)]
pub struct InitAccumulator<'info> {
    #[account(mut)]
    pub admin: Signer<'info>,
    #[account(seeds = [CONFIG_SEED], bump = config.bump, has_one = admin @ LcError::NotAdmin)]
    pub config: Box<Account<'info, Config>>,
    #[account(init, payer = admin, space = acc_layout::LEN, seeds = [ACC_SEED], bump)]
    pub acc: AccountLoader<'info, Accumulator>,
    pub system_program: Program<'info, System>,
}

#[derive(Accounts)]
pub struct Accumulate<'info> {
    #[account(seeds = [CONFIG_SEED], bump = config.bump)]
    pub config: Box<Account<'info, Config>>,
    #[account(seeds = [CANON_SEED], bump)]
    pub canon: AccountLoader<'info, Canon>,
    #[account(mut, seeds = [ACC_SEED], bump = acc.load()?.bump)]
    pub acc: AccountLoader<'info, Accumulator>,
}

#[derive(Accounts)]
pub struct ProveHeader<'info> {
    #[account(seeds = [ACC_SEED], bump = acc.load()?.bump)]
    pub acc: AccountLoader<'info, Accumulator>,
}

#[derive(Accounts)]
#[instruction(hash: [u8; 32])]
pub struct RestoreHeader<'info> {
    #[account(mut)]
    pub payer: Signer<'info>,
    #[account(seeds = [ACC_SEED], bump = acc.load()?.bump)]
    pub acc: AccountLoader<'info, Accumulator>,
    #[account(init, payer = payer, space = layout::LEN, seeds = [HEADER_SEED, &hash], bump)]
    pub header: AccountLoader<'info, HeaderRecord>,
    pub system_program: Program<'info, System>,
}

#[derive(Accounts)]
pub struct CloseHeader<'info> {
    #[account(seeds = [CONFIG_SEED], bump = config.bump)]
    pub config: Box<Account<'info, Config>>,
    #[account(seeds = [CANON_SEED], bump)]
    pub canon: AccountLoader<'info, Canon>,
    #[account(seeds = [ACC_SEED], bump = acc.load()?.bump)]
    pub acc: AccountLoader<'info, Accumulator>,
    #[account(mut, close = relayer)]
    pub header: AccountLoader<'info, HeaderRecord>,
    /// CHECK: receives the rent; must be the header's relayer.
    #[account(mut)]
    pub relayer: UncheckedAccount<'info>,
}

// ---------------------------------------------------------------- events, errors

#[event]
pub struct HeaderSubmitted {
    pub hash: [u8; 32],
    pub height: u64,
}

#[event]
pub struct HeaderVerified {
    pub hash: [u8; 32],
    pub height: u64,
}

#[event]
pub struct WorkVerified {
    pub hash: [u8; 32],
    pub height: u64,
}

#[event]
pub struct TipChanged {
    pub hash: [u8; 32],
    pub height: u64,
    pub reorg_depth: u64,
}

#[event]
pub struct HeaderFinal {
    pub hash: [u8; 32],
    pub height: u64,
}

#[event]
pub struct Accumulated {
    pub height: u64,
    pub hash: [u8; 32],
    pub count: u64,
    pub root: [u8; 32],
}

#[event]
pub struct HeaderRestored {
    pub hash: [u8; 32],
    pub height: u64,
}

#[event]
pub struct JobRegistered {
    pub job_id: [u8; 32],
    pub posted_height: u64,
}

#[error_code]
pub enum LcError {
    #[msg("invalid parameters")]
    BadParams,
    #[msg("only the admin may do this")]
    NotAdmin,
    #[msg("hash does not match the header bytes")]
    HashMismatch,
    #[msg("bad header (version, parent link or height)")]
    BadHeader,
    #[msg("parent header is not verified")]
    ParentNotVerified,
    #[msg("at or below the final floor")]
    TooDeep,
    #[msg("target_bits is not the WTEMA target")]
    WrongTarget,
    #[msg("timestamp rule violated")]
    Timestamp,
    #[msg("header is not pending")]
    NotPending,
    #[msg("header is not verified")]
    NotVerified,
    #[msg("job block needs its registered job")]
    JobMissing,
    #[msg("bad work account")]
    BadWork,
    #[msg("work account is bound to another header")]
    WrongTarget2,
    #[msg("proof bytes are frozen")]
    Frozen,
    #[msg("proof does not fit the work account")]
    ProofTooLarge,
    #[msg("verification steps out of order")]
    BadStep,
    #[msg("proof verification failed")]
    VerifyFailed,
    #[msg("branch is not heavier than the tip")]
    NotHeavier,
    #[msg("bad reorg path")]
    BadPath,
    #[msg("header is not below the final floor")]
    NotFinal,
    #[msg("header is not canonical")]
    NotCanonical,
    #[msg("not the header's relayer")]
    WrongRelayer,
    #[msg("header cannot be closed yet")]
    CannotClose,
    #[msg("header is not the next one for the accumulator")]
    AccOrder,
    #[msg("final header is not in the accumulator yet")]
    NotAccumulated,
    #[msg("accumulator size is not current or recent")]
    AccRoot,
    #[msg("accumulator proof failed")]
    AccProof,
    #[msg("header is not WorkDone")]
    NotWorkDone,
}
