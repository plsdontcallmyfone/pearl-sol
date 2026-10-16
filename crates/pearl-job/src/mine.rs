//! CPU share mining on a job's committed matrices.
//!
//! Adapted from Pearl `zk-pow/src/ffi/mine.rs::try_mine_one`, ISC: takes the job's A
//! and B instead of random ones, and uses the miner-bound seed.

use std::ops::Range;

use anyhow::{ensure, Result};

use crate::job::{as_u8, commit_a, difficulty_bound, derive_seeds, meets_target, transpose, Job, SeedDerivation};
use crate::merkle::{pad_to_chunk_boundary, MerkleTree};
use crate::noise::{jackpot_hash, jackpot_noised, noise_for_indices};
use crate::proof::PlainProof;

/// A job plus its matrices, with Merkle trees built once.
pub struct PreparedJob {
    pub job: Job,
    pub job_key: [u8; 32],
    /// A, m x k row-major.
    pub a: Vec<i8>,
    /// B^T, n x k row-major.
    pub bt: Vec<i8>,
    a_tree: MerkleTree,
    bt_tree: MerkleTree,
}

impl PreparedJob {
    /// `a` is m x k row-major, `b` is k x n row-major. Fails if the matrices do not
    /// match the job's commitments or values fall outside int7 [-64, 64].
    pub fn new(job: Job, a: &[i8], b: &[i8]) -> Result<Self> {
        job.sanity_check()?;
        let (m, n, k) = (job.m as usize, job.n as usize, job.config.k());
        ensure!(a.len() == m * k, "A has {} entries, expected m*k = {}", a.len(), m * k);
        ensure!(b.len() == k * n, "B has {} entries, expected k*n = {}", b.len(), k * n);
        ensure!(a.iter().chain(b).all(|&x| (-64..=64).contains(&x)), "matrix values must be int7 in [-64, 64]");
        let job_key = job.job_key();
        let bt = transpose(b, k, n);
        let a_tree = MerkleTree::new(&pad_to_chunk_boundary(as_u8(a)), job_key);
        let bt_tree = MerkleTree::new(&pad_to_chunk_boundary(as_u8(&bt)), job_key);
        ensure!(a_tree.root() == job.hash_a, "A does not match the job's hash_a");
        ensure!(bt_tree.root() == job.hash_b, "B does not match the job's hash_b");
        debug_assert_eq!(a_tree.root(), commit_a(&job_key, a));
        Ok(Self { job, job_key, a: a.to_vec(), bt, a_tree, bt_tree })
    }

    /// Try one nonce: full noised GEMM, scanning tiles in order. Returns the first
    /// tile whose jackpot hash meets `share_target` (compact nbits).
    pub fn mine_nonce(&self, miner: &[u8; 32], nonce: u64, share_target: u32) -> Option<PlainProof> {
        let c = &self.job.config;
        let (m, n, k, r) = (self.job.m as usize, self.job.n as usize, c.k(), c.r());
        let (b_seed, a_seed) = derive_seeds(
            SeedDerivation::MinerBound { miner: *miner, nonce },
            &self.job_key,
            &self.job.hash_a,
            &self.job.hash_b,
            self.job.m,
            self.job.n,
        );
        let all_a: Vec<usize> = (0..m).collect();
        let all_b: Vec<usize> = (0..n).collect();
        let noise = noise_for_indices(k, r, (&b_seed, &a_seed), &all_a, &all_b);
        let a_n: Vec<i32> =
            (0..m).flat_map(|i| (0..k).map(move |l| (i, l))).map(|(i, l)| self.a[i * k + l] as i32 + noise.a[i][l] as i32).collect();
        let b_n: Vec<i32> =
            (0..n).flat_map(|j| (0..k).map(move |l| (j, l))).map(|(j, l)| self.bt[j * k + l] as i32 + noise.bt[j][l] as i32).collect();
        let bound = difficulty_bound(share_target, c);
        let row_groups = c.rows_pattern.partition(m).ok()?;
        let col_groups = c.cols_pattern.partition(n).ok()?;
        for rows in &row_groups {
            let ar: Vec<&[i32]> = rows.iter().map(|&i| &a_n[i * k..(i + 1) * k]).collect();
            for cols in &col_groups {
                let br: Vec<&[i32]> = cols.iter().map(|&j| &b_n[j * k..(j + 1) * k]).collect();
                let msg = jackpot_noised(k, r, &ar, &br);
                let h = jackpot_hash(&msg, &a_seed);
                if meets_target(&h, &bound) {
                    return Some(self.build_proof(nonce, rows, cols));
                }
            }
        }
        None
    }

    /// First winning nonce in `nonces`.
    pub fn mine_range(&self, miner: &[u8; 32], nonces: Range<u64>, share_target: u32) -> Option<PlainProof> {
        nonces.into_iter().find_map(|nonce| self.mine_nonce(miner, nonce, share_target))
    }

    fn build_proof(&self, nonce: u64, rows: &[usize], cols: &[usize]) -> PlainProof {
        let k = self.job.config.k();
        PlainProof {
            m: self.job.m,
            n: self.job.n,
            k: k as u32,
            rank: self.job.config.rank,
            nonce,
            t_rows: rows[0] as u32,
            t_cols: cols[0] as u32,
            a: self.a_tree.multileaf_proof(&MerkleTree::leaf_indices_for_rows(rows, k)),
            bt: self.bt_tree.multileaf_proof(&MerkleTree::leaf_indices_for_rows(cols, k)),
        }
    }

    /// The useful result C = AB (m x n, i32 row-major).
    pub fn product(&self) -> Vec<i32> {
        crate::freivalds::matmul_bt(&self.a, &self.bt, self.job.m as usize, self.job.n as usize, self.job.config.k())
    }
}
