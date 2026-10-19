//! pearl-job: Pearl-style useful proof of work bound to an on-chain job.
//!
//! - job header (SPEC 3.3) and `job_key = blake3(header76 || config52)`
//! - Pearl-format commitments `hash_a`, `hash_b` (keyed BLAKE3 of the int7 matrices)
//! - miner-bound noise seeds (SPEC 3.4, NOTES-proof Q4)
//! - CPU share mining on the job's own A, B; native plain-proof verify
//! - Freivalds check of a delivered C = AB
//! - SPEC section 6 signed message bytes
//! - v2 block proofs (`pow`): 256-byte header, pow_digest-bound seed, fallback blocks
//!
//! Large parts are ported from Pearl (github.com/pearl-research-labs/pearl @ 837c69c,
//! crates `zk-pow` and `pearl-blake3`), ISC licensed. See NOTICE.

pub mod data;
pub mod freivalds;
pub mod job;
pub mod merkle;
pub mod messages;
pub mod mine;
pub mod noise;
pub mod pattern;
pub mod pow;
pub mod proof;
pub mod spec;
pub mod standing;
pub mod verify;

use std::ops::Range;

use anyhow::Result;

pub use job::{commit_a, commit_b, job_key, Job, JobHeader, MiningConfig, SeedDerivation};
pub use mine::PreparedJob;
pub use pow::{verify_block, verify_block_proof, BlockHeader, BlockWork, FallbackParams, MinedBlock};
pub use proof::PlainProof;
pub use spec::JobSpec;
pub use verify::{verify_share, verify_share_with_target, verify_with_seed_mode, VerifiedShare};

/// `(hash_a, hash_b)` for A (m x k) and B (k x n), both row-major int7.
pub fn commit(job_key: &[u8; 32], a: &[i8], b: &[i8], k: usize, n: usize) -> ([u8; 32], [u8; 32]) {
    (commit_a(job_key, a), commit_b(job_key, b, k, n))
}

/// Mine the first share in `nonces` for `miner` on the job's matrices.
pub fn mine_share(
    a: &[i8],
    b: &[i8],
    job: &Job,
    miner: &[u8; 32],
    share_target: u32,
    nonces: Range<u64>,
) -> Result<Option<PlainProof>> {
    let prep = PreparedJob::new(job.clone(), a, b)?;
    Ok(prep.mine_range(miner, nonces, share_target))
}
