//! Native plain-proof verification (no ZK).
//!
//! Follows Pearl `zk-pow/src/api/verify.rs::verify_plain_proof`, ISC, with the
//! config and commitments taken from the job instead of from the proof.

use anyhow::{ensure, Result};

use crate::job::{derive_seeds, difficulty_bound, meets_target, Job, SeedDerivation};
use crate::merkle::{padded_chunk_len, MerkleProof, MerkleTree, CHUNK_LEN};
use crate::noise::{jackpot, jackpot_hash, noise_for_indices};
use crate::pattern::PeriodicPattern;
use crate::proof::PlainProof;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerifiedShare {
    pub nonce: u64,
    pub t_rows: u32,
    pub t_cols: u32,
    pub jackpot_hash: [u8; 32],
}

fn tile_indices(p: &PeriodicPattern, t: u32, dim: u32, what: &str) -> Result<Vec<usize>> {
    ensure!(p.offset_is_valid(t), "{what} offset {t} is not a valid tile start");
    ensure!(t as u64 + (p.max() as u64) < dim as u64, "{what} tile at {t} runs past dimension {dim}");
    Ok(p.indices_with_offset(t).into_iter().map(|x| x as usize).collect())
}

fn open_strips(
    proof: &MerkleProof,
    rows: &[usize],
    total_rows: usize,
    k: usize,
    dlen: usize,
    key: &[u8; 32],
    root: &[u8; 32],
    what: &str,
) -> Result<Vec<Vec<i8>>> {
    let want_leaves = padded_chunk_len(total_rows * k) / CHUNK_LEN;
    ensure!(proof.total_leaves == want_leaves, "{what}: tree declares {} leaves, dims imply {want_leaves}", proof.total_leaves);
    ensure!(
        proof.leaf_indices == MerkleTree::leaf_indices_for_rows(rows, k),
        "{what}: opened leaves are not exactly the tile's leaves"
    );
    ensure!(proof.compute_root(key).as_ref() == Some(root), "{what}: Merkle opening does not match the job commitment");
    rows.iter()
        .map(|&row| {
            let bytes = proof.extract_bytes(row * k, dlen)?;
            let strip: Vec<i8> = bytes.into_iter().map(|x| x as i8).collect();
            ensure!(strip.iter().all(|v| (-64..=64).contains(v)), "{what}: value out of int7 range");
            Ok(strip)
        })
        .collect()
}

/// Verify a share for `miner` against the job's own share target (`header.target_bits`).
pub fn verify_share(job: &Job, proof: &PlainProof, miner: &[u8; 32]) -> Result<VerifiedShare> {
    verify_share_with_target(job, proof, miner, job.header.target_bits)
}

pub fn verify_share_with_target(job: &Job, proof: &PlainProof, miner: &[u8; 32], share_target: u32) -> Result<VerifiedShare> {
    let mode = SeedDerivation::MinerBound { miner: *miner, nonce: proof.nonce };
    verify_with_seed_mode(job, proof, mode, share_target)
}

/// Verify under any seed derivation. `MinerBound` (v1 shares) and `PowBound` (v2
/// blocks, via `pow::verify_block_proof`) are the chain modes; the other
/// modes exist to cross-check this port against Pearl's own verifier.
pub fn verify_with_seed_mode(job: &Job, proof: &PlainProof, mode: SeedDerivation, share_target: u32) -> Result<VerifiedShare> {
    job.sanity_check()?;
    let c = &job.config;
    ensure!(proof.m == job.m && proof.n == job.n, "proof dims do not match the job");
    ensure!(proof.k == c.common_dim && proof.rank == c.rank, "proof k / rank do not match the job config");
    let (k, r, dlen) = (c.k(), c.r(), c.dot_product_length());
    let rows = tile_indices(&c.rows_pattern, proof.t_rows, job.m, "row")?;
    let cols = tile_indices(&c.cols_pattern, proof.t_cols, job.n, "col")?;
    let key = job.job_key();
    let s_a = open_strips(&proof.a, &rows, job.m as usize, k, dlen, &key, &job.hash_a, "A")?;
    let s_bt = open_strips(&proof.bt, &cols, job.n as usize, k, dlen, &key, &job.hash_b, "B^T")?;
    let (b_seed, a_seed) = derive_seeds(
        mode,
        &key,
        &job.hash_a,
        &job.hash_b,
        job.m,
        job.n,
    );
    let noise = noise_for_indices(k, r, (&b_seed, &a_seed), &rows, &cols);
    let msg = jackpot(k, r, &s_a, &s_bt, &noise);
    let h = jackpot_hash(&msg, &a_seed);
    ensure!(meets_target(&h, &difficulty_bound(share_target, c)), "jackpot hash does not meet the share target");
    Ok(VerifiedShare { nonce: proof.nonce, t_rows: proof.t_rows, t_cols: proof.t_cols, jackpot_hash: h })
}
