//! Off-chain helpers (not compiled for SBF): PDAs, instruction builders and the
//! verify-step plan the relay and the tests use. CU per step was measured in
//! token/tests/lightclient-litesvm (see `plan` and the HANDOFF numbers).

use anchor_lang::solana_program::instruction::{AccountMeta, Instruction};
use anchor_lang::solana_program::pubkey::Pubkey;
use anchor_lang::solana_program::system_program;
use anchor_lang::{InstructionData, ToAccountMetas};

use crate::{accounts, instruction, Params, CANON_SEED, CONFIG_SEED, HEADER_SEED, JOB_SEED, POOL_SEED};

pub fn config_pda() -> Pubkey {
    Pubkey::find_program_address(&[CONFIG_SEED], &crate::ID).0
}
pub fn canon_pda() -> Pubkey {
    Pubkey::find_program_address(&[CANON_SEED], &crate::ID).0
}
pub fn pool_pda() -> Pubkey {
    Pubkey::find_program_address(&[POOL_SEED], &crate::ID).0
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

pub fn verify_proof_hash(authority: Pubkey, work: Pubkey, hash: &[u8; 32]) -> Instruction {
    ix(step_accounts(authority, work, hash), instruction::VerifyProofHash {})
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
pub fn verify_noise(authority: Pubkey, work: Pubkey, hash: &[u8; 32], side: u8) -> Instruction {
    ix(step_accounts(authority, work, hash), instruction::VerifyNoise { side })
}
pub fn verify_jackpot(authority: Pubkey, work: Pubkey, hash: &[u8; 32], relayer: Pubkey) -> Instruction {
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
        instruction::VerifyJackpot {},
    )
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

pub fn close_header(hash: &[u8; 32], relayer: Pubkey) -> Instruction {
    ix(accounts::CloseHeader { config: config_pda(), canon: canon_pda(), header: header_addr(hash), relayer }, instruction::CloseHeader {})
}

/// Compute-unit budget used to pack steps (each tx asks for 1.4M; keep headroom).
pub const TX_CU_BUDGET: u64 = 1_300_000;

/// Measured CU (token/tests/lightclient-litesvm, localnet config k=1024 r=32 4x8
/// tiles, 64 x 64 fallback); used to pack steps into transactions.
pub mod cost {
    // measured (rounded up about 3%) on the 2026-10-17 build
    pub const PROOF_HASH: u64 = 665_000;
    pub const GEN_B_PER_BLOCK: u64 = 4_850;
    pub const HASH_BT_PER_CHUNK: u64 = 53_500;
    pub const HASH_A_PER_CHUNK: u64 = 116_000;
    pub const OPENING_A: u64 = 255_000;
    pub const OPENING_BT: u64 = 495_000;
    pub const NOISE_A: u64 = 710_000;
    pub const NOISE_BT: u64 = 865_000;
    pub const JACKPOT: u64 = 690_000;
    pub const STEP_OVERHEAD: u64 = 15_000;
}

/// One verify step.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Step {
    ProofHash,
    GenB(u32),
    HashBt(u32),
    HashA(u32),
    Opening(u8),
    Noise(u8),
    Jackpot,
}

impl Step {
    pub fn cost(&self) -> u64 {
        use cost::*;
        STEP_OVERHEAD
            + match *self {
                Step::ProofHash => PROOF_HASH,
                Step::GenB(n) => GEN_B_PER_BLOCK * n as u64,
                Step::HashBt(n) => HASH_BT_PER_CHUNK * n as u64,
                Step::HashA(n) => HASH_A_PER_CHUNK * n as u64,
                Step::Opening(0) => OPENING_A,
                Step::Opening(_) => OPENING_BT,
                Step::Noise(0) => NOISE_A,
                Step::Noise(_) => NOISE_BT,
                Step::Jackpot => JACKPOT,
            }
    }

    pub fn instruction(&self, authority: Pubkey, work: Pubkey, hash: &[u8; 32], relayer: Pubkey) -> Instruction {
        match *self {
            Step::ProofHash => verify_proof_hash(authority, work, hash),
            Step::GenB(n) => fallback_gen_b(authority, work, hash, n),
            Step::HashBt(n) => fallback_hash_bt(authority, work, hash, n),
            Step::HashA(n) => fallback_hash_a(authority, work, hash, n),
            Step::Opening(s) => verify_opening(authority, work, hash, s),
            Step::Noise(s) => verify_noise(authority, work, hash, s),
            Step::Jackpot => verify_jackpot(authority, work, hash, relayer),
        }
    }
}

/// Steps for one header packed greedily into transactions of at most `budget` CU:
/// each tx takes as many units of the current step kind as fit, then continues with
/// the next kind (fallback ranges are split across txs as needed).
pub fn plan(fallback: bool, fb_m: u32, fb_n: u32, fb_k: u32, budget: u64) -> Vec<Vec<Step>> {
    use cost::*;
    // (constructor, units, cost per unit)
    let mut kinds: Vec<(fn(u32) -> Step, u64, u64)> = vec![(|_| Step::ProofHash, 1, PROOF_HASH)];
    if fallback {
        kinds.push((Step::GenB, (fb_k as u64 * fb_n as u64) / 64, GEN_B_PER_BLOCK));
        kinds.push((Step::HashBt, (fb_k as u64 * fb_n as u64).div_ceil(1024), HASH_BT_PER_CHUNK));
        kinds.push((Step::HashA, (fb_m as u64 * fb_k as u64).div_ceil(1024), HASH_A_PER_CHUNK));
    }
    kinds.push((|_| Step::Opening(0), 1, OPENING_A));
    kinds.push((|_| Step::Opening(1), 1, OPENING_BT));
    kinds.push((|_| Step::Noise(0), 1, NOISE_A));
    kinds.push((|_| Step::Noise(1), 1, NOISE_BT));
    kinds.push((|_| Step::Jackpot, 1, JACKPOT));
    let mut txs: Vec<Vec<Step>> = Vec::new();
    let mut cur: Vec<Step> = Vec::new();
    let mut left = budget;
    for (make, mut units, per) in kinds {
        while units > 0 {
            let fit = left.saturating_sub(STEP_OVERHEAD) / per;
            if fit == 0 {
                if cur.is_empty() {
                    // a single unit above the budget still goes alone
                    txs.push(vec![make(1)]);
                    units -= 1;
                    continue;
                }
                txs.push(std::mem::take(&mut cur));
                left = budget;
                continue;
            }
            let n = fit.min(units);
            cur.push(make(n as u32));
            left -= STEP_OVERHEAD + n * per;
            units -= n;
        }
    }
    if !cur.is_empty() {
        txs.push(cur);
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
