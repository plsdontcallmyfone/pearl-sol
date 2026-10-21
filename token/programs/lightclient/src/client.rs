//! Off-chain helpers (not compiled for SBF): PDAs, instruction builders and the
//! verify-step plan the relay and the tests use. CU per step was measured in
//! token/tests/lightclient-litesvm (see `plan` and the HANDOFF numbers).

use anchor_lang::solana_program::instruction::{AccountMeta, Instruction};
use anchor_lang::solana_program::pubkey::Pubkey;
use anchor_lang::solana_program::system_program;
use anchor_lang::{InstructionData, ToAccountMetas};

use crate::{accounts, instruction, Params, ACC_SEED, CANON_SEED, CONFIG_SEED, HEADER_SEED, JOB_SEED, POOL_SEED};

pub fn config_pda() -> Pubkey {
    Pubkey::find_program_address(&[CONFIG_SEED], &crate::ID).0
}
pub fn canon_pda() -> Pubkey {
    Pubkey::find_program_address(&[CANON_SEED], &crate::ID).0
}
pub fn pool_pda() -> Pubkey {
    Pubkey::find_program_address(&[POOL_SEED], &crate::ID).0
}
pub fn acc_addr() -> Pubkey {
    Pubkey::find_program_address(&[ACC_SEED], &crate::ID).0
}
pub fn header_addr(hash: &[u8; 32]) -> Pubkey {
    Pubkey::find_program_address(&[HEADER_SEED, hash], &crate::ID).0
}
pub fn job_addr(job_id: &[u8; 32]) -> Pubkey {
    Pubkey::find_program_address(&[JOB_SEED, job_id], &crate::ID).0
}

/// Size of a work account for proofs up to `proof_cap` bytes and fallback B of `k*n` bytes.
pub fn work_size(proof_cap: usize, fallback_k: usize, fallback_n: usize) -> usize {
    crate::work::PROOF_OFF + proof_cap + fallback_k * fallback_n
}

fn ix(accounts: impl ToAccountMetas, data: impl InstructionData) -> Instruction {
    Instruction { program_id: crate::ID, accounts: accounts.to_account_metas(None), data: data.data() }
}

pub fn initialize(admin: Pubkey, params: Params, checkpoint: [u8; 256], cum_work: [u8; 32], ts_hist: Vec<u64>) -> Instruction {
    let hash = pearl_verify_sbf::blake3::hash(&checkpoint);
    ix(
        accounts::Initialize {
            admin,
            config: config_pda(),
            canon: canon_pda(),
            header: header_addr(&hash),
            pool: pool_pda(),
            system_program: system_program::ID,
        },
        instruction::Initialize { checkpoint_hash: hash, params, checkpoint, cum_work, ts_hist },
    )
}

pub fn set_params(admin: Pubkey, params: Params) -> Instruction {
    ix(accounts::AdminOnly { admin, config: config_pda() }, instruction::SetParams { params })
}

pub fn submit_header(payer: Pubkey, header: [u8; 256]) -> Instruction {
    let hash = pearl_verify_sbf::blake3::hash(&header);
    let prev: [u8; 32] = header[12..44].try_into().unwrap();
    ix(
        accounts::SubmitHeader {
            payer,
            config: config_pda(),
            parent: header_addr(&prev),
            header: header_addr(&hash),
            system_program: system_program::ID,
        },
        instruction::SubmitHeader { hash, header },
    )
}

pub fn init_work(authority: Pubkey, work: Pubkey, proof_cap: u32) -> Instruction {
    ix(accounts::InitWork { authority, work }, instruction::InitWork { proof_cap })
}

pub fn begin_verify(authority: Pubkey, work: Pubkey, hash: &[u8; 32], job_id: Option<&[u8; 32]>, proof_len: u32) -> Instruction {
    let mut i = ix(
        accounts::BeginVerify { authority, work, config: config_pda(), header: header_addr(hash) },
        instruction::BeginVerify { proof_len },
    );
    if let Some(j) = job_id {
        i.accounts.push(AccountMeta::new_readonly(job_addr(j), false));
    }
    i
}

pub fn write_proof(authority: Pubkey, work: Pubkey, offset: u32, data: Vec<u8>) -> Instruction {
    ix(accounts::WorkOnly { authority, work }, instruction::WriteProof { offset, data })
}

fn step_accounts(authority: Pubkey, work: Pubkey, hash: &[u8; 32]) -> accounts::VerifyStep {
    accounts::VerifyStep { authority, work, header: header_addr(hash) }
}

pub fn verify_proof_hash(authority: Pubkey, work: Pubkey, hash: &[u8; 32], count: u32) -> Instruction {
    ix(step_accounts(authority, work, hash), instruction::VerifyProofHash { count })
}
pub fn fallback_gen_b(authority: Pubkey, work: Pubkey, hash: &[u8; 32], count: u32) -> Instruction {
    ix(step_accounts(authority, work, hash), instruction::FallbackGenB { count })
}
pub fn fallback_hash_bt(authority: Pubkey, work: Pubkey, hash: &[u8; 32], count: u32) -> Instruction {
    ix(step_accounts(authority, work, hash), instruction::FallbackHashBt { count })
}
pub fn fallback_hash_a(authority: Pubkey, work: Pubkey, hash: &[u8; 32], count: u32) -> Instruction {
    ix(step_accounts(authority, work, hash), instruction::FallbackHashA { count })
}
pub fn verify_opening(authority: Pubkey, work: Pubkey, hash: &[u8; 32], side: u8) -> Instruction {
    ix(step_accounts(authority, work, hash), instruction::VerifyOpening { side })
}
pub fn verify_noise(authority: Pubkey, work: Pubkey, hash: &[u8; 32], side: u8, count: u32) -> Instruction {
    ix(step_accounts(authority, work, hash), instruction::VerifyNoise { side, count })
}
pub fn verify_jackpot(authority: Pubkey, work: Pubkey, hash: &[u8; 32], relayer: Pubkey, count: u32) -> Instruction {
    ix(
        accounts::VerifyFinal {
            authority,
            work,
            header: header_addr(hash),
            config: config_pda(),
            canon: canon_pda(),
            pool: pool_pda(),
            relayer,
        },
        instruction::VerifyJackpot { count },
    )
}

/// `verify_jackpot` with the parent header as the remaining account, so a header whose
/// parent is Verified but not the tip (a side branch) is marked Verified right away.
pub fn verify_jackpot_with_parent(authority: Pubkey, work: Pubkey, hash: &[u8; 32], parent: &[u8; 32], relayer: Pubkey, count: u32) -> Instruction {
    let mut i = verify_jackpot(authority, work, hash, relayer, count);
    i.accounts.push(AccountMeta::new_readonly(header_addr(parent), false));
    i
}

/// Promote a WorkDone header (`hash`) whose parent (`parent`) is Verified.
pub fn connect(parent: &[u8; 32], hash: &[u8; 32], relayer: Pubkey) -> Instruction {
    ix(
        accounts::Connect {
            config: config_pda(),
            canon: canon_pda(),
            pool: pool_pda(),
            parent: header_addr(parent),
            header: header_addr(hash),
            relayer,
        },
        instruction::Connect {},
    )
}

/// Close a work account (lamports back to `authority`).
pub fn close_work(authority: Pubkey, work: Pubkey) -> Instruction {
    ix(accounts::CloseWork { authority, work }, instruction::CloseWork {})
}

/// `path` = the new tip's ancestors newest first, ending with the canonical fork point.
pub fn reorg(new_tip: &[u8; 32], path: &[[u8; 32]]) -> Instruction {
    let mut i = ix(accounts::Reorg { config: config_pda(), canon: canon_pda(), new_tip: header_addr(new_tip) }, instruction::Reorg {});
    for h in path {
        i.accounts.push(AccountMeta::new_readonly(header_addr(h), false));
    }
    i
}

pub fn mark_final(hash: &[u8; 32]) -> Instruction {
    ix(accounts::MarkFinal { config: config_pda(), canon: canon_pda(), header: header_addr(hash) }, instruction::MarkFinal {})
}

/// `mark_final` of a header that left the canonical ring: `child` is its final child.
pub fn mark_final_linked(hash: &[u8; 32], child: &[u8; 32]) -> Instruction {
    let mut i = mark_final(hash);
    i.accounts.push(AccountMeta::new_readonly(header_addr(child), false));
    i
}

/// Re-register `job_id` from `tx` in `block` when its current commit's block is dead;
/// `witness` = the final or canonical header at the old commit's posted height.
pub fn replace_job(payer: Pubkey, block: &[u8; 32], witness: &[u8; 32], job_id: [u8; 32], tx: Vec<u8>, path: Vec<u8>) -> Instruction {
    ix(
        accounts::ReplaceJob {
            payer,
            config: config_pda(),
            canon: canon_pda(),
            block: header_addr(block),
            witness: header_addr(witness),
            job: job_addr(&job_id),
        },
        instruction::ReplaceJob { job_id, tx, path },
    )
}

pub fn register_job(payer: Pubkey, block: &[u8; 32], job_id: [u8; 32], tx: Vec<u8>, path: Vec<u8>) -> Instruction {
    ix(
        accounts::RegisterJob {
            payer,
            config: config_pda(),
            canon: canon_pda(),
            block: header_addr(block),
            job: job_addr(&job_id),
            system_program: system_program::ID,
        },
        instruction::RegisterJob { job_id, tx, path },
    )
}

/// `begin_verify` for a job block with its job proven inline (SPEC 5.3): `block` is the
/// header that posted the job, `tx` the posting chain tx, `path` its tx_root path.
pub fn begin_verify_job(authority: Pubkey, work: Pubkey, hash: &[u8; 32], block: &[u8; 32], proof_len: u32, tx: Vec<u8>, path: Vec<u8>) -> Instruction {
    ix(
        accounts::BeginVerifyJob { authority, work, config: config_pda(), canon: canon_pda(), header: header_addr(hash), block: header_addr(block) },
        instruction::BeginVerifyJob { proof_len, tx, path },
    )
}

pub fn close_header(hash: &[u8; 32], relayer: Pubkey) -> Instruction {
    ix(accounts::CloseHeader { config: config_pda(), canon: canon_pda(), acc: acc_addr(), header: header_addr(hash), relayer }, instruction::CloseHeader {})
}

/// R5: start the final-header accumulator at the current final header (admin, once).
pub fn init_accumulator(admin: Pubkey) -> Instruction {
    ix(accounts::InitAccumulator { admin, config: config_pda(), acc: acc_addr(), system_program: system_program::ID }, instruction::InitAccumulator {})
}

/// R5: append final headers (hashes in height order, starting at the accumulator's next height).
pub fn accumulate(hashes: &[[u8; 32]]) -> Instruction {
    let mut i = ix(accounts::Accumulate { config: config_pda(), canon: canon_pda(), acc: acc_addr() }, instruction::Accumulate {});
    i.accounts.extend(hashes.iter().map(|h| AccountMeta::new_readonly(header_addr(h), false)));
    i
}

/// R5: prove a final header by accumulator inclusion (return data: hash || height LE).
pub fn prove_header(header: [u8; 256], count: u64, proof: Vec<u8>) -> Instruction {
    ix(accounts::ProveHeader { acc: acc_addr() }, instruction::ProveHeader { header, count, proof })
}

/// R5: re-create a closed final header's record from an accumulator proof.
pub fn restore_header(payer: Pubkey, header: [u8; 256], count: u64, proof: Vec<u8>) -> Instruction {
    let hash = pearl_verify_sbf::blake3::hash(&header);
    ix(
        accounts::RestoreHeader { payer, acc: acc_addr(), header: header_addr(&hash), system_program: system_program::ID },
        instruction::RestoreHeader { hash, header, count, proof },
    )
}

/// Accumulator state as raw fields: `(start_height, count, last_hash)`.
pub fn acc_state(data: &[u8]) -> Option<(u64, u64, [u8; 32])> {
    use crate::acc_layout::*;
    if data.len() < LEN {
        return None;
    }
    let rd = |o: usize| u64::from_le_bytes(data[o..o + 8].try_into().unwrap());
    Some((rd(START_HEIGHT), rd(COUNT), data[LAST_HASH..LAST_HASH + 32].try_into().unwrap()))
}

/// Compute-unit budget used to pack steps (each tx asks for 1.4M; keep headroom).
pub const TX_CU_BUDGET: u64 = 1_300_000;

/// Measured CU (token/tests/lightclient-litesvm, localnet config k=2048 r=128 4x8
/// tiles, after the rank-penalty port); used to pack steps into transactions.
pub mod cost {
    // measured (rounded up a few percent) on the 2026-10-17 rank-penalty build
    pub const PROOF_HASH_PER_CHUNK: u64 = 54_000;
    /// parse + structure checks in the call that completes the proof hash
    pub const STRUCTURE: u64 = 30_000;
    pub const GEN_B_PER_BLOCK: u64 = 4_850;
    pub const HASH_BT_PER_CHUNK: u64 = 53_500;
    pub const HASH_A_PER_CHUNK: u64 = 116_000;
    pub const OPENING_PER_LEAF: u64 = 62_000;
    /// per call: seeds, uniform noise rows, strip copy; per row
    pub const NOISE_CALL_PER_ROW: u64 = 17_000;
    pub const NOISE_CALL: u64 = 32_000;
    /// per 8-column block: one permutation hash plus 8 adds per row
    pub const NOISE_BLOCK: u64 = 4_150;
    pub const NOISE_BLOCK_PER_ROW: u64 = 290;
    /// per rank block: h*w dot products of length r
    pub const JACKPOT_PER_MAC: u64 = 17;
    pub const JACKPOT_CALL: u64 = 60_000;
    pub const STEP_OVERHEAD: u64 = 15_000;
}

/// One verify step (counts are units of resumable steps).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Step {
    /// 1 KiB chunks of the proof hash (the last call adds the structure checks).
    ProofHash(u32),
    GenB(u32),
    HashBt(u32),
    HashA(u32),
    Opening(u8),
    /// (side, 8-column noise blocks)
    Noise(u8, u32),
    /// rank blocks
    Jackpot(u32),
}

impl Step {
    pub fn instruction(&self, authority: Pubkey, work: Pubkey, hash: &[u8; 32], relayer: Pubkey) -> Instruction {
        match *self {
            Step::ProofHash(n) => verify_proof_hash(authority, work, hash, n),
            Step::GenB(n) => fallback_gen_b(authority, work, hash, n),
            Step::HashBt(n) => fallback_hash_bt(authority, work, hash, n),
            Step::HashA(n) => fallback_hash_a(authority, work, hash, n),
            Step::Opening(s) => verify_opening(authority, work, hash, s),
            Step::Noise(s, n) => verify_noise(authority, work, hash, s, n),
            Step::Jackpot(n) => verify_jackpot(authority, work, hash, relayer, n),
        }
    }

    /// As `instruction`, with the parent passed to the jackpot step (see
    /// `verify_jackpot_with_parent`).
    pub fn instruction_with_parent(&self, authority: Pubkey, work: Pubkey, hash: &[u8; 32], parent: &[u8; 32], relayer: Pubkey) -> Instruction {
        match *self {
            Step::Jackpot(n) => verify_jackpot_with_parent(authority, work, hash, parent, relayer, n),
            _ => self.instruction(authority, work, hash, relayer),
        }
    }
}

/// What the plan needs about the block being verified.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Shape {
    pub fallback: bool,
    pub fb_m: u32,
    pub fb_n: u32,
    pub k: u32,
    pub r: u32,
    pub h: u32,
    pub w: u32,
    pub proof_len: u32,
}

impl Shape {
    /// From the mining config the block is verified against (the fallback config for
    /// fallback blocks, the job's config for job blocks) and the proof length.
    pub fn new(fallback: bool, fb_m: u32, fb_n: u32, config: &[u8; 52], proof_len: usize) -> Self {
        let c = pearl_verify_sbf::config::MiningConfig::from_bytes(config).expect("valid mining config");
        Self { fallback, fb_m, fb_n, k: c.k, r: c.rank, h: c.h(), w: c.w(), proof_len: proof_len as u32 }
    }
    fn dlen(&self) -> u64 {
        (self.k - self.k % self.r) as u64
    }
}

/// Steps for one header packed greedily into transactions of at most `budget` CU:
/// each tx takes as many units of the current step kind as fit, then continues with
/// the next kind (resumable kinds are split across txs as needed).
pub fn plan(s: &Shape, budget: u64) -> Vec<Vec<Step>> {
    plan_costed(s, budget).into_iter().map(|(t, _)| t).collect()
}

/// `plan` with each transaction's estimated CU (from `cost`), so a relay can ask for a
/// compute limit close to what the tx uses (priority fees are charged on the limit).
pub fn plan_costed(s: &Shape, budget: u64) -> Vec<(Vec<Step>, u64)> {
    use cost::*;
    let leaves = |rows: u32| rows as u64 * (s.k as u64).div_ceil(1024);
    let (dlen, k) = (s.dlen(), s.k as u64);
    // (constructor, units, cost per unit, fixed cost per call)
    type Make = Box<dyn Fn(u32) -> Step>;
    let mut kinds: Vec<(Make, u64, u64, u64)> =
        vec![(Box::new(Step::ProofHash), (s.proof_len as u64).div_ceil(1024).max(1), PROOF_HASH_PER_CHUNK, STRUCTURE)];
    if s.fallback {
        let (m, n) = (s.fb_m as u64, s.fb_n as u64);
        kinds.push((Box::new(Step::GenB), (k * n) / 64, GEN_B_PER_BLOCK, 0));
        kinds.push((Box::new(Step::HashBt), (k * n).div_ceil(1024), HASH_BT_PER_CHUNK, 0));
        kinds.push((Box::new(Step::HashA), (m * k).div_ceil(1024), HASH_A_PER_CHUNK, 0));
    }
    kinds.push((Box::new(|_| Step::Opening(0)), 1, OPENING_PER_LEAF * leaves(s.h), 0));
    kinds.push((Box::new(|_| Step::Opening(1)), 1, OPENING_PER_LEAF * leaves(s.w), 0));
    for (side, rows) in [(0u8, s.h as u64), (1, s.w as u64)] {
        kinds.push((
            Box::new(move |n| Step::Noise(side, n)),
            dlen / 8,
            NOISE_BLOCK + NOISE_BLOCK_PER_ROW * rows,
            NOISE_CALL + NOISE_CALL_PER_ROW * rows,
        ));
    }
    kinds.push((Box::new(Step::Jackpot), dlen / s.r as u64, JACKPOT_PER_MAC * (s.h * s.w * s.r) as u64, JACKPOT_CALL));
    let mut txs: Vec<(Vec<Step>, u64)> = Vec::new();
    let mut cur: Vec<Step> = Vec::new();
    let mut left = budget;
    for (make, mut units, per, fixed) in kinds {
        while units > 0 {
            let fit = left.saturating_sub(STEP_OVERHEAD + fixed) / per.max(1);
            if fit == 0 {
                if cur.is_empty() {
                    // a single unit above the budget still goes alone
                    txs.push((vec![make(1)], STEP_OVERHEAD + fixed + per));
                    units -= 1;
                    continue;
                }
                txs.push((std::mem::take(&mut cur), budget - left));
                left = budget;
                continue;
            }
            let n = fit.min(units);
            cur.push(make(n as u32));
            left -= STEP_OVERHEAD + fixed + n * per;
            units -= n;
        }
    }
    if !cur.is_empty() {
        txs.push((cur, budget - left));
    }
    txs
}

/// Proof bytes per `write_proof` tx (legacy tx limit 1232 bytes with a CU-limit ix).
pub const WRITE_CHUNK: usize = 950;
/// Proof bytes in the `begin_verify` tx.
pub const FIRST_CHUNK: usize = 700;

/// Work account size for these params: 32 KiB of proof, then scratch for the larger of
/// fallback B (k*n) and the noised strips of a localnet 4x8 tile (12 x k bytes).
pub fn work_size_for(p: &Params) -> usize {
    let k = u32::from_le_bytes(p.fallback_config[0..4].try_into().unwrap()) as usize;
    work_size(32 * 1024, k, (p.fallback_n as usize).max(16))
}
