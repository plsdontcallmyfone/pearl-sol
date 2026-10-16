//! The useful result: C = AB, its commitment, and a Freivalds check.

use rand::Rng;

use crate::merkle::blake3_digest;

/// C = A B with A m x k row-major and B^T n x k row-major. i32 cannot overflow:
/// |a*b| <= 64*64 and k <= 2^16 gives |c| <= 2^28.
pub fn matmul_bt(a: &[i8], bt: &[i8], m: usize, n: usize, k: usize) -> Vec<i32> {
    let mut c = vec![0i32; m * n];
    for i in 0..m {
        let ar = &a[i * k..(i + 1) * k];
        for j in 0..n {
            let br = &bt[j * k..(j + 1) * k];
            let mut s = 0i32;
            for l in 0..k {
                s += ar[l] as i32 * br[l] as i32;
            }
            c[i * n + j] = s;
        }
    }
    c
}

/// C = A B with B k x n row-major.
pub fn matmul(a: &[i8], b: &[i8], m: usize, n: usize, k: usize) -> Vec<i32> {
    matmul_bt(a, &crate::job::transpose(b, k, n), m, n, k)
}

pub fn c_to_bytes(c: &[i32]) -> Vec<u8> {
    c.iter().flat_map(|x| x.to_le_bytes()).collect()
}

/// `result_hash = blake3_keyed(C as i32 LE row-major, key = job_key)`.
pub fn result_hash(job_key: &[u8; 32], c: &[i32]) -> [u8; 32] {
    blake3_digest(&c_to_bytes(c), Some(job_key))
}

const P: u64 = (1u64 << 61) - 1;

fn red(x: i64) -> u64 {
    (x.rem_euclid(P as i64)) as u64
}

fn mulmod(a: u64, b: u64) -> u64 {
    ((a as u128 * b as u128) % P as u128) as u64
}

fn addmod(a: u64, b: u64) -> u64 {
    let s = a + b;
    if s >= P {
        s - P
    } else {
        s
    }
}

/// One Freivalds round with challenge vector `r` (length n): checks A(Br) == Cr mod p.
pub fn freivalds_round(a: &[i8], b: &[i8], c: &[i32], m: usize, n: usize, k: usize, r: &[u64]) -> bool {
    let mut br = vec![0u64; k];
    for l in 0..k {
        let row = &b[l * n..(l + 1) * n];
        let mut s = 0u64;
        for j in 0..n {
            s = addmod(s, mulmod(red(row[j] as i64), r[j]));
        }
        br[l] = s;
    }
    for i in 0..m {
        let mut lhs = 0u64;
        for l in 0..k {
            lhs = addmod(lhs, mulmod(red(a[i * k + l] as i64), br[l]));
        }
        let mut rhs = 0u64;
        for j in 0..n {
            rhs = addmod(rhs, mulmod(red(c[i * n + j] as i64), r[j]));
        }
        if lhs != rhs {
            return false;
        }
    }
    true
}

/// Freivalds over GF(2^61 - 1) with uniformly random challenges. A wrong C passes one
/// round with probability at most 1/p, about 2^-61. `rng` must be unpredictable to
/// the deliverer (the attester uses the OS RNG).
pub fn freivalds<R: Rng>(a: &[i8], b: &[i8], c: &[i32], m: usize, n: usize, k: usize, rounds: usize, rng: &mut R) -> bool {
    if a.len() != m * k || b.len() != k * n || c.len() != m * n {
        return false;
    }
    (0..rounds.max(1)).all(|_| {
        let r: Vec<u64> = (0..n).map(|_| rng.random_range(0..P)).collect();
        freivalds_round(a, b, c, m, n, k, &r)
    })
}
