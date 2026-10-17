//! v2 block proofs (SPEC v2 section 3): the 256-byte block header, `pow_digest`,
//! `proof_hash`, the pow-bound seed and fallback blocks. Byte layouts are in
//! PROOF-FORMAT.md section 7.
//!
//! A block proof is a PSJ1 plain proof with seed mode 3. The noise seeds come from
//! the block's `pow_digest` (header with `proof_hash` zeroed), so a proof is bound to
//! one header: one parent, one payout key, one nonce, one set of roots. The block's
//! difficulty is the header's `target_bits`, not the job's share target.

use std::ops::Range;

use anyhow::{bail, ensure, Result};

use crate::job::{Hash256, Job, JobHeader, MiningConfig, SeedDerivation};
use crate::merkle::blake3_digest;
use crate::mine::PreparedJob;
use crate::proof::{PlainProof, SEED_MODE_POW_BOUND};
use crate::verify::{verify_with_seed_mode, VerifiedShare};

pub const HEADER_LEN: usize = 256;

/// Byte offsets of the header fields (all integers little-endian).
pub mod off {
    pub const VERSION: usize = 0;
    pub const HEIGHT: usize = 4;
    pub const PREV_HASH: usize = 12;
    pub const TIMESTAMP_MS: usize = 44;
    pub const TARGET_BITS: usize = 52;
    pub const JOB_ID: usize = 56;
    pub const MINER: usize = 88;
    pub const TX_ROOT: usize = 120;
    pub const STATE_ROOT: usize = 152;
    pub const WITHDRAW_ROOT: usize = 184;
    pub const PROOF_HASH: usize = 216;
    pub const NONCE: usize = 248;
}

/// SPEC v2 3.1 block header, fixed 256 bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct BlockHeader {
    pub version: u32,
    pub height: u64,
    pub prev_hash: Hash256,
    pub timestamp_ms: u64,
    /// Chain target (Bitcoin compact), the block's difficulty.
    pub target_bits: u32,
    /// Zero = fallback block.
    pub job_id: Hash256,
    /// Payout key.
    pub miner: Hash256,
    pub tx_root: Hash256,
    pub state_root: Hash256,
    pub withdraw_root: Hash256,
    /// `blake3(proof bytes)`.
    pub proof_hash: Hash256,
    pub nonce: u64,
}

fn arr(b: &[u8], at: usize) -> Hash256 {
    b[at..at + 32].try_into().unwrap()
}

impl BlockHeader {
    pub fn to_bytes(&self) -> [u8; HEADER_LEN] {
        let mut b = [0u8; HEADER_LEN];
        b[off::VERSION..off::VERSION + 4].copy_from_slice(&self.version.to_le_bytes());
        b[off::HEIGHT..off::HEIGHT + 8].copy_from_slice(&self.height.to_le_bytes());
        b[off::PREV_HASH..off::PREV_HASH + 32].copy_from_slice(&self.prev_hash);
        b[off::TIMESTAMP_MS..off::TIMESTAMP_MS + 8].copy_from_slice(&self.timestamp_ms.to_le_bytes());
        b[off::TARGET_BITS..off::TARGET_BITS + 4].copy_from_slice(&self.target_bits.to_le_bytes());
        b[off::JOB_ID..off::JOB_ID + 32].copy_from_slice(&self.job_id);
        b[off::MINER..off::MINER + 32].copy_from_slice(&self.miner);
        b[off::TX_ROOT..off::TX_ROOT + 32].copy_from_slice(&self.tx_root);
        b[off::STATE_ROOT..off::STATE_ROOT + 32].copy_from_slice(&self.state_root);
        b[off::WITHDRAW_ROOT..off::WITHDRAW_ROOT + 32].copy_from_slice(&self.withdraw_root);
        b[off::PROOF_HASH..off::PROOF_HASH + 32].copy_from_slice(&self.proof_hash);
        b[off::NONCE..off::NONCE + 8].copy_from_slice(&self.nonce.to_le_bytes());
        b
    }

    pub fn from_bytes(b: &[u8]) -> Result<Self> {
        ensure!(b.len() == HEADER_LEN, "block header must be {HEADER_LEN} bytes, got {}", b.len());
        let u32_at = |o: usize| u32::from_le_bytes(b[o..o + 4].try_into().unwrap());
        let u64_at = |o: usize| u64::from_le_bytes(b[o..o + 8].try_into().unwrap());
        Ok(Self {
            version: u32_at(off::VERSION),
            height: u64_at(off::HEIGHT),
            prev_hash: arr(b, off::PREV_HASH),
            timestamp_ms: u64_at(off::TIMESTAMP_MS),
            target_bits: u32_at(off::TARGET_BITS),
            job_id: arr(b, off::JOB_ID),
            miner: arr(b, off::MINER),
            tx_root: arr(b, off::TX_ROOT),
            state_root: arr(b, off::STATE_ROOT),
            withdraw_root: arr(b, off::WITHDRAW_ROOT),
            proof_hash: arr(b, off::PROOF_HASH),
            nonce: u64_at(off::NONCE),
        })
    }

    /// `blake3(header)`.
    pub fn block_hash(&self) -> Hash256 {
        blake3_digest(&self.to_bytes(), None)
    }

    /// `blake3(header with proof_hash zeroed)`: what the proof binds to.
    pub fn pow_digest(&self) -> Hash256 {
        pow_digest(&self.to_bytes())
    }

    pub fn is_fallback(&self) -> bool {
        self.job_id == [0u8; 32]
    }
}

/// `blake3(header bytes with bytes 216..248 zeroed)`.
pub fn pow_digest(header: &[u8; HEADER_LEN]) -> Hash256 {
    let mut h = *header;
    h[off::PROOF_HASH..off::PROOF_HASH + 32].fill(0);
    blake3_digest(&h, None)
}

/// `blake3(proof bytes)`.
pub fn proof_hash(proof: &[u8]) -> Hash256 {
    blake3_digest(proof, None)
}

/// Serialise a block proof (PSJ1, seed mode 3).
pub fn block_proof_to_bytes(p: &PlainProof) -> Vec<u8> {
    p.to_bytes_with_mode(SEED_MODE_POW_BOUND)
}

/// Parse a block proof (PSJ1, seed mode 3 only).
pub fn block_proof_from_bytes(b: &[u8]) -> Result<PlainProof> {
    PlainProof::from_bytes_with_mode(b, SEED_MODE_POW_BOUND)
}

// ---------------------------------------------------------------- job blocks

impl PreparedJob {
    /// One header nonce on this job: full noised GEMM under the pow-bound seed.
    /// The returned proof carries `nonce` (it must equal the header's nonce).
    pub fn mine_block(&self, pow_digest: &Hash256, target_bits: u32, nonce: u64) -> Option<PlainProof> {
        let seeds = self.seeds(SeedDerivation::PowBound { pow_digest: *pow_digest });
        self.find_tile(seeds, target_bits).map(|(rows, cols)| self.build_proof(nonce, &rows, &cols))
    }
}

/// Mine one header nonce on a job's own matrices (`a` m x k, `b` k x n, row-major).
/// `job` carries the 76-byte job header, the config and the commitments.
/// Builds the Merkle trees on every call; a miner loop should keep a [`PreparedJob`]
/// and call [`PreparedJob::mine_block`].
pub fn mine_block_proof(a: &[i8], b: &[i8], job: &Job, pow_digest: &Hash256, target_bits: u32, nonce: u64) -> Result<Option<PlainProof>> {
    let prep = PreparedJob::new(job.clone(), a, b)?;
    Ok(prep.mine_block(pow_digest, target_bits, nonce))
}

/// Check a block proof against a job, a `pow_digest`, the chain target and the
/// header nonce. Does not check `proof_hash`; [`verify_block`] does the whole header.
pub fn verify_block_proof(job: &Job, proof: &PlainProof, pow_digest: &Hash256, target_bits: u32, nonce: u64) -> Result<VerifiedShare> {
    ensure!(proof.nonce == nonce, "proof nonce {} is not the header nonce {nonce}", proof.nonce);
    verify_with_seed_mode(job, proof, SeedDerivation::PowBound { pow_digest: *pow_digest }, target_bits)
}

// ---------------------------------------------------------------- fallback blocks

/// Shape of a fallback block's matrices (chain parameters `fallback_m`, `fallback_n`,
/// `fallback_config`; default 4 x 8, one 4x8 tile of the localnet config, owner
/// decision 2026-10-17: small enough for the Solana light client, k stays 1024).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FallbackParams {
    pub m: u32,
    pub n: u32,
    pub config: MiningConfig,
}

impl Default for FallbackParams {
    fn default() -> Self {
        Self { m: 4, n: 8, config: MiningConfig::localnet_default() }
    }
}

pub const FALLBACK_CTX_A: &str = "pearl-sol 2026-10-16 fallback matrix A v1";
pub const FALLBACK_CTX_B: &str = "pearl-sol 2026-10-16 fallback matrix B v1";

fn xof_int7(ctx: &str, pow_digest: &Hash256, len: usize) -> Vec<i8> {
    let mut h = blake3::Hasher::new_derive_key(ctx);
    h.update(pow_digest);
    let mut buf = vec![0u8; len];
    h.finalize_xof().fill(&mut buf);
    buf.into_iter().map(|x| (x & 0x7f) as i8 - 64).collect()
}

/// The 76-byte job header of a fallback block: `0^32 || pow_digest || 0 u64 || 0 u32`.
pub fn fallback_job_header(pow_digest: &Hash256) -> JobHeader {
    JobHeader { job_pubkey: [0u8; 32], terms_hash: *pow_digest, created_slot: 0, target_bits: 0 }
}

/// Fallback matrices: `A = int7(blake3_derive_key(FALLBACK_CTX_A, pow_digest) XOF, m*k)`
/// row-major, `B = int7(... FALLBACK_CTX_B ..., k*n)` row-major (k x n), with
/// `int7(x) = (x & 0x7f) - 64`.
pub fn fallback_matrices(pow_digest: &Hash256, p: &FallbackParams) -> (Vec<i8>, Vec<i8>) {
    let (m, n, k) = (p.m as usize, p.n as usize, p.config.k());
    (xof_int7(FALLBACK_CTX_A, pow_digest, m * k), xof_int7(FALLBACK_CTX_B, pow_digest, k * n))
}

/// The fallback block's job (with commitments filled in) and its matrices.
pub fn fallback_job(pow_digest: &Hash256, p: &FallbackParams) -> Result<PreparedJob> {
    let (a, b) = fallback_matrices(pow_digest, p);
    let job = Job { header: fallback_job_header(pow_digest), config: p.config, m: p.m, n: p.n, hash_a: [0; 32], hash_b: [0; 32] };
    PreparedJob::with_computed_commitments(job, &a, &b)
}

// ---------------------------------------------------------------- whole headers

/// What a block is mined on.
pub enum BlockWork<'a> {
    /// A posted job: its prepared matrices; the header's `job_id` names it on the chain.
    /// The job's Pearl header id may differ from that chain id (storefront orders use the
    /// poster, docs/RPC.md), so the caller pairs the two; the proof binds both (job_key via
    /// the seeds, the chain job_id via `pow_digest`).
    Job(&'a PreparedJob),
    /// `job_id` zero: matrices from `pow_digest`.
    Fallback(FallbackParams),
}

/// A found block: the header (with `nonce` and `proof_hash` set) and the proof bytes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MinedBlock {
    pub header: BlockHeader,
    pub proof: Vec<u8>,
}

/// Try one nonce on `template` (all fields but `nonce` and `proof_hash` final).
pub fn mine_header_nonce(template: &BlockHeader, work: &BlockWork, nonce: u64) -> Result<Option<MinedBlock>> {
    let mut h = *template;
    h.nonce = nonce;
    h.proof_hash = [0; 32];
    let pd = h.pow_digest();
    let proof = match work {
        BlockWork::Job(prep) => {
            ensure!(!h.is_fallback(), "job blocks need a non-zero job_id");
            prep.mine_block(&pd, h.target_bits, nonce)
        }
        BlockWork::Fallback(p) => {
            ensure!(h.is_fallback(), "fallback blocks need job_id zero");
            fallback_job(&pd, p)?.mine_block(&pd, h.target_bits, nonce)
        }
    };
    Ok(proof.map(|p| {
        let bytes = block_proof_to_bytes(&p);
        h.proof_hash = proof_hash(&bytes);
        MinedBlock { header: h, proof: bytes }
    }))
}

/// First winning nonce in `nonces`.
pub fn mine_header(template: &BlockHeader, work: &BlockWork, nonces: Range<u64>) -> Result<Option<MinedBlock>> {
    for nonce in nonces {
        if let Some(b) = mine_header_nonce(template, work, nonce)? {
            return Ok(Some(b));
        }
    }
    Ok(None)
}

/// Full check of a block's work: `proof_hash`, the proof's seed mode, the job binding,
/// the pow-bound seed and the header's target. `job` is the posted job named by
/// `header.job_id` (ignored, and may be None, for fallback blocks); the caller looks it up
/// by that id (its Pearl header id need not equal it, see [`BlockWork::Job`]).
pub fn verify_block(header: &BlockHeader, job: Option<&Job>, proof: &[u8], fallback: &FallbackParams) -> Result<VerifiedShare> {
    ensure!(proof_hash(proof) == header.proof_hash, "proof_hash does not match the proof bytes");
    let p = block_proof_from_bytes(proof)?;
    let pd = header.pow_digest();
    if header.is_fallback() {
        let fj = fallback_job(&pd, fallback)?;
        return verify_block_proof(&fj.job, &p, &pd, header.target_bits, header.nonce);
    }
    let Some(job) = job else { bail!("job block needs its job") };
    verify_block_proof(job, &p, &pd, header.target_bits, header.nonce)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn header_layout_and_digests() {
        let h = BlockHeader {
            version: 1,
            height: 0x0102030405060708,
            prev_hash: [0x11; 32],
            timestamp_ms: 0x1122334455667788,
            target_bits: 0x1e040000,
            job_id: [0x22; 32],
            miner: [0x33; 32],
            tx_root: [0x44; 32],
            state_root: [0x55; 32],
            withdraw_root: [0x66; 32],
            proof_hash: [0x77; 32],
            nonce: 0xaabbccddeeff0011,
        };
        let b = h.to_bytes();
        assert_eq!(off::NONCE + 8, HEADER_LEN);
        assert_eq!(&b[0..4], &[1, 0, 0, 0]);
        assert_eq!(&b[4..12], &[8, 7, 6, 5, 4, 3, 2, 1]);
        assert_eq!(&b[52..56], &0x1e040000u32.to_le_bytes());
        assert_eq!(&b[216..248], &[0x77; 32]);
        assert_eq!(BlockHeader::from_bytes(&b).unwrap(), h);
        let mut z = h;
        z.proof_hash = [0; 32];
        assert_eq!(h.pow_digest(), *blake3::hash(&z.to_bytes()).as_bytes());
        assert_eq!(h.pow_digest(), z.pow_digest());
        assert_eq!(h.block_hash(), *blake3::hash(&b).as_bytes());
        assert_ne!(h.block_hash(), z.block_hash());
        assert!(BlockHeader::from_bytes(&b[1..]).is_err());
    }

    #[test]
    fn fallback_derivation_is_deterministic() {
        let p = FallbackParams::default();
        let (a1, b1) = fallback_matrices(&[9; 32], &p);
        let (a2, b2) = fallback_matrices(&[9; 32], &p);
        assert_eq!((a1.clone(), b1.clone()), (a2, b2));
        assert_eq!(a1.len(), 4 * 2048);
        assert_eq!(b1.len(), 2048 * 8);
        assert!(a1.iter().chain(&b1).all(|&x| (-64..=63).contains(&x)));
        let (a3, _) = fallback_matrices(&[8; 32], &p);
        assert_ne!(a1, a3);
        assert_ne!(a1[..2048], b1[..2048]);
        let j1 = fallback_job(&[9; 32], &p).unwrap();
        let j2 = fallback_job(&[9; 32], &p).unwrap();
        assert_eq!(j1.job, j2.job);
        assert_ne!(j1.job.hash_a, [0; 32]);
    }
}
