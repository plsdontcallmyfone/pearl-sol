//! Low-rank noise and the tile jackpot.
//!
//! Ported from Pearl `zk-pow/src/circuit/pearl_noise.rs` and
//! `zk-pow/src/circuit/chip/jackpot/helper.rs`, ISC. Single-threaded (deterministic).

use crate::merkle::blake3_digest;

const NOISE_RANGE: usize = 128;
const IDXS_PER_COL: usize = 2;
const UNIFORM_NOISE_RANGE: usize = NOISE_RANGE / IDXS_PER_COL;
const ZERO_POINT: i8 = (UNIFORM_NOISE_RANGE / 2) as i8;
const RANGE_MASK: u8 = (UNIFORM_NOISE_RANGE - 1) as u8;
const DIGEST: usize = 32;

pub const JACKPOT_SIZE: usize = 16;
pub const LROT_PER_TILE: u32 = 13;

const fn label(l: [u8; 8]) -> [u8; 32] {
    let mut r = [0u8; 32];
    let mut i = 0;
    while i < 8 {
        r[i] = l[i];
        i += 1;
    }
    r
}
const SEED_LABEL_A: [u8; 32] = label(*b"A_tensor");
const SEED_LABEL_B: [u8; 32] = label(*b"B_tensor");

/// Noise rows of A (h x k) and noise rows of B^T (w x k) for the requested indices.
#[derive(Debug, Clone)]
pub struct NoiseSlice {
    pub a: Vec<Vec<i8>>,
    pub bt: Vec<Vec<i8>>,
}

fn random_hash(index: usize, seed: &[u8; 32], key: &[u8; 32], prepend: usize) -> [u8; 32] {
    let mut msg = [0u8; 64];
    msg[prepend * 4..prepend * 4 + 4].copy_from_slice(&((1 + index) as i32).to_le_bytes());
    msg[32..].copy_from_slice(seed);
    blake3_digest(&msg, Some(key))
}

fn uniform_rows(seed: &[u8; 32], key: &[u8; 32], rows: &[usize], cols: usize) -> Vec<Vec<i8>> {
    rows.iter()
        .map(|&row| {
            let start = row * cols;
            let mut out = Vec::with_capacity(cols);
            for block in start / DIGEST..(start + cols).div_ceil(DIGEST) {
                let h = random_hash(block, seed, key, 0);
                for (j, &byte) in h.iter().enumerate() {
                    let idx = block * DIGEST + j;
                    if idx >= start && idx < start + cols {
                        out.push((byte & RANGE_MASK) as i8 - ZERO_POINT);
                    }
                }
            }
            out
        })
        .collect()
}

fn permutation(seed: &[u8; 32], key: &[u8; 32], k: usize, rank: usize) -> Vec<[u32; 2]> {
    let mask = (rank - 1) as u32;
    let mut res = vec![[0u32; 2]; k];
    for (i, chunk) in res.chunks_mut(DIGEST / 4).enumerate() {
        let h = random_hash(i, seed, key, 1);
        for (j, slot) in chunk.iter_mut().enumerate() {
            let x = u32::from_le_bytes(h[j * 4..j * 4 + 4].try_into().unwrap());
            let first = x & mask;
            let second = first ^ (1 + (((rank - 1) as u64 * x as u64) >> 32) as u32);
            *slot = [first, second];
        }
    }
    res
}

fn sparse_matvec(perm: &[[u32; 2]], v: &[i8]) -> Vec<i8> {
    perm.iter().map(|&[p, q]| (v[p as usize] as i32 - v[q as usize] as i32) as i8).collect()
}

/// Pearl `compute_noise_for_indices`. `seeds = (b_noise_seed, a_noise_seed)`.
pub fn noise_for_indices(k: usize, rank: usize, seeds: (&[u8; 32], &[u8; 32]), a_rows: &[usize], bt_rows: &[usize]) -> NoiseSlice {
    let (b_seed, a_seed) = seeds;
    let e_al = uniform_rows(&SEED_LABEL_A, a_seed, a_rows, rank);
    let e_ar_t = permutation(&SEED_LABEL_A, a_seed, k, rank);
    let e_bl = permutation(&SEED_LABEL_B, b_seed, k, rank);
    let e_br_t = uniform_rows(&SEED_LABEL_B, b_seed, bt_rows, rank);
    NoiseSlice {
        a: e_al.iter().map(|row| sparse_matvec(&e_ar_t, row)).collect(),
        bt: e_br_t.iter().map(|col| sparse_matvec(&e_bl, col)).collect(),
    }
}

/// The four noise factors behind [`noise_for_indices`], for analysis (the rank-penalty
/// work-ratio tests): `E_A = e_al * e_ar_t^T`, `E_B^T = e_br_t * e_bl^T`. The sparse
/// factors are `k` pairs `[p, q]` meaning `+1` at column p and `-1` at column q.
#[derive(Debug, Clone)]
pub struct NoiseFactors {
    pub e_al: Vec<Vec<i8>>,
    pub e_ar_t: Vec<[u32; 2]>,
    pub e_bl: Vec<[u32; 2]>,
    pub e_br_t: Vec<Vec<i8>>,
}

pub fn noise_factors(k: usize, rank: usize, seeds: (&[u8; 32], &[u8; 32]), a_rows: &[usize], bt_rows: &[usize]) -> NoiseFactors {
    let (b_seed, a_seed) = seeds;
    NoiseFactors {
        e_al: uniform_rows(&SEED_LABEL_A, a_seed, a_rows, rank),
        e_ar_t: permutation(&SEED_LABEL_A, a_seed, k, rank),
        e_bl: permutation(&SEED_LABEL_B, b_seed, k, rank),
        e_br_t: uniform_rows(&SEED_LABEL_B, b_seed, bt_rows, rank),
    }
}

/// Pearl `compute_jackpot`: running h x w partial products of the noised strips,
/// XOR-folded into 16 words after every rank block. Strips have length >= dlen.
pub fn jackpot(k: usize, r: usize, s_a: &[Vec<i8>], s_bt: &[Vec<i8>], noise: &NoiseSlice) -> [u32; JACKPOT_SIZE] {
    let a: Vec<Vec<i32>> =
        s_a.iter().zip(&noise.a).map(|(s, n)| s.iter().zip(n).map(|(&x, &e)| x as i32 + e as i32).collect()).collect();
    let b: Vec<Vec<i32>> =
        s_bt.iter().zip(&noise.bt).map(|(s, n)| s.iter().zip(n).map(|(&x, &e)| x as i32 + e as i32).collect()).collect();
    jackpot_noised(k, r, &a.iter().map(|v| &v[..]).collect::<Vec<_>>(), &b.iter().map(|v| &v[..]).collect::<Vec<_>>())
}

/// Same as [`jackpot`] on already-noised rows (A+E rows, (B+F)^T rows).
pub fn jackpot_noised(k: usize, r: usize, a: &[&[i32]], bt: &[&[i32]]) -> [u32; JACKPOT_SIZE] {
    let (h, w) = (a.len(), bt.len());
    let mut acc = vec![0i32; h * w];
    let mut msg = [0u32; JACKPOT_SIZE];
    let mut ll = r;
    while ll <= k {
        for u in 0..h {
            let ar = &a[u][ll - r..ll];
            for v in 0..w {
                let br = &bt[v][ll - r..ll];
                let mut s = 0i32;
                for l in 0..r {
                    s = s.wrapping_add(ar[l].wrapping_mul(br[l]));
                }
                acc[u * w + v] = acc[u * w + v].wrapping_add(s);
            }
        }
        let x = acc.iter().fold(0u32, |a, &x| a ^ x as u32);
        let tid = (ll / r - 1) % JACKPOT_SIZE;
        msg[tid] = msg[tid].rotate_left(LROT_PER_TILE) ^ x;
        ll += r;
    }
    msg
}

/// `blake3(jackpot words LE, key = a_noise_seed)`.
pub fn jackpot_hash(msg: &[u32; JACKPOT_SIZE], a_noise_seed: &[u8; 32]) -> [u8; 32] {
    let mut b = [0u8; 64];
    for (i, w) in msg.iter().enumerate() {
        b[i * 4..i * 4 + 4].copy_from_slice(&w.to_le_bytes());
    }
    blake3_digest(&b, Some(a_noise_seed))
}
