//! Plain Pearl block-proof verification, split into independent steps so the
//! light client can spread it over several instructions. Ported from
//! crates/pearl-job/src/{verify,merkle,noise,job}.rs (PROOF-FORMAT.md sections 4, 7).
//!
//! Steps (each checks one claim about the same immutable proof bytes):
//! 1. [`check_structure`]: dims, config, tile offsets, exact leaf index sets.
//! 2. [`check_opening`] for A and for B^T: Merkle root equals the job commitment and
//!    every opened value is int7.
//! 3. [`check_jackpot`]: pow-bound seeds, low-rank noise, tile jackpot, target.

extern crate alloc;
use alloc::vec;
use alloc::vec::Vec;

use crate::blake3::{self, Cv, KEYED_HASH};
use crate::config::{MiningConfig, Pattern};
use crate::difficulty::{difficulty_bound, meets_target};
use crate::proof::{Opening, ProofView, LEAF};
use crate::Error;

/// `blake3("pearl/cert-v3/noise-seed/A")`.
pub const SEED_SALT_A: [u8; 32] = [
    0x82, 0x49, 0x40, 0x6c, 0xa0, 0xed, 0x15, 0x16, 0x96, 0x16, 0xf6, 0x92, 0xfc, 0xf0, 0x76, 0xf8, 0x92, 0xdb, 0xdb,
    0x2a, 0x70, 0x23, 0xb8, 0x52, 0xf0, 0xd4, 0x77, 0x19, 0xc3, 0x90, 0x01, 0x7b,
];
/// `blake3("pearl/cert-v3/noise-seed/B")`.
pub const SEED_SALT_B: [u8; 32] = [
    0x11, 0x30, 0x06, 0x32, 0xec, 0x63, 0x01, 0xca, 0x2b, 0xe2, 0xaf, 0x71, 0x8b, 0x3f, 0x4d, 0x4f, 0x1a, 0xe9, 0xc6,
    0x39, 0x88, 0xe8, 0xcc, 0x04, 0x48, 0x44, 0x30, 0x1d, 0x71, 0xb8, 0x9a, 0xa9,
];

const LABEL_A: [u8; 32] = label(*b"A_tensor");
const LABEL_B: [u8; 32] = label(*b"B_tensor");
const JACKPOT_SIZE: usize = 16;

const fn label(l: [u8; 8]) -> [u8; 32] {
    let mut r = [0u8; 32];
    let mut i = 0;
    while i < 8 {
        r[i] = l[i];
        i += 1;
    }
    r
}

/// What the verifier needs about the job a block is mined on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct JobParams {
    pub job_key: [u8; 32],
    pub config: MiningConfig,
    pub m: u32,
    pub n: u32,
    pub hash_a: [u8; 32],
    pub hash_b: [u8; 32],
}

/// `job_key = blake3(header76 || config52)`.
pub fn job_key(header76: &[u8; 76], config: &[u8; 52]) -> [u8; 32] {
    let mut d = [0u8; 128];
    d[..76].copy_from_slice(header76);
    d[76..].copy_from_slice(config);
    blake3::hash(&d)
}

fn tile_ok(p: &Pattern, t: u32, dim: u32) -> bool {
    p.offset_is_valid(t) && (t as u64 + p.max() as u64) < dim as u64
}

/// Leaves covering rows `pattern + t` of a row-major (rows x k) byte matrix, sorted,
/// unique, compared one by one with the opening's index list.
fn check_leaf_set(p: &[u8], o: &Opening, pat: &Pattern, t: u32, total_rows: u32, k: u32) -> Result<(), Error> {
    let want_total = (total_rows as u64 * k as u64).div_ceil(LEAF as u64);
    if o.total_leaves as u64 != want_total {
        return Err(Error::Opening);
    }
    let mut i = 0usize;
    let mut last: Option<u64> = None;
    for r in 0..pat.size() {
        let row = (pat.index(r) + t) as u64;
        let first = row * k as u64 / LEAF as u64;
        let end = ((row + 1) * k as u64 - 1) / LEAF as u64;
        for leaf in first..=end {
            if last.is_some_and(|l| leaf <= l) {
                continue;
            }
            if i >= o.count || o.leaf_index(p, i) as u64 != leaf {
                return Err(Error::Opening);
            }
            last = Some(leaf);
            i += 1;
        }
    }
    if i != o.count {
        return Err(Error::Opening);
    }
    Ok(())
}

/// Step 1: everything about the proof that needs no hashing.
pub fn check_structure(p: &[u8], v: &ProofView, job: &JobParams, header_nonce: u64) -> Result<(), Error> {
    let c = &job.config;
    c.check_shape(job.m, job.n)?;
    if v.nonce != header_nonce {
        return Err(Error::Nonce);
    }
    if v.m != job.m || v.n != job.n || v.k != c.k || v.rank as u32 != c.rank {
        return Err(Error::Dims);
    }
    if !tile_ok(&c.rows, v.t_rows, job.m) || !tile_ok(&c.cols, v.t_cols, job.n) {
        return Err(Error::Tile);
    }
    check_leaf_set(p, &v.a, &c.rows, v.t_rows, job.m, c.k)?;
    check_leaf_set(p, &v.bt, &c.cols, v.t_cols, job.n, c.k)?;
    Ok(())
}

/// Root of a multi-leaf opening (`merkle::MerkleProof::compute_root`).
pub fn opening_root(p: &[u8], o: &Opening, key: &[u8; 32]) -> Result<[u8; 32], Error> {
    if o.count == 0 {
        return Err(Error::Opening);
    }
    for i in 1..o.count {
        if o.leaf_index(p, i - 1) >= o.leaf_index(p, i) {
            return Err(Error::Opening);
        }
    }
    if o.leaf_index(p, o.count - 1) >= o.total_leaves {
        return Err(Error::Opening);
    }
    if o.total_leaves == 1 {
        if o.sib_count != 0 {
            return Err(Error::Opening);
        }
        return Ok(blake3::keyed_hash(key, o.leaf(p, 0)));
    }
    let kw = blake3::key_words(key);
    let mut cur: Vec<(u32, Cv)> = Vec::with_capacity(o.count);
    for i in 0..o.count {
        let idx = o.leaf_index(p, i);
        cur.push((idx, blake3::chunk_output(&kw, KEYED_HASH, o.leaf(p, i), idx as u64).chaining_value()));
    }
    let mut sib = 0usize;
    let next_sib = |sib: &mut usize| -> Result<Cv, Error> {
        if *sib >= o.sib_count {
            return Err(Error::Opening);
        }
        let s = o.sibling(p, *sib);
        *sib += 1;
        Ok(blake3::key_words(&s))
    };
    let mut level_len = o.total_leaves;
    while level_len > 2 {
        let mut next: Vec<(u32, Cv)> = Vec::with_capacity(cur.len());
        let mut j = 0;
        while j < cur.len() {
            let (i, cv) = cur[j];
            if i % 2 == 0 {
                let right = if j + 1 < cur.len() && cur[j + 1].0 == i + 1 {
                    j += 1;
                    Some(cur[j].1)
                } else if i + 1 < level_len {
                    Some(next_sib(&mut sib)?)
                } else {
                    None
                };
                next.push((i / 2, match right {
                    Some(r) => blake3::parent_cv(&kw, KEYED_HASH, &cv, &r),
                    None => cv,
                }));
            } else {
                let left = next_sib(&mut sib)?;
                next.push((i / 2, blake3::parent_cv(&kw, KEYED_HASH, &left, &cv)));
            }
            j += 1;
        }
        cur = next;
        level_len = level_len.div_ceil(2);
    }
    let get = |idx: u32, cur: &[(u32, Cv)]| cur.iter().find(|(i, _)| *i == idx).map(|(_, c)| *c);
    let l = match get(0, &cur) {
        Some(v) => v,
        None => next_sib(&mut sib)?,
    };
    let r = match get(1, &cur) {
        Some(v) => v,
        None => next_sib(&mut sib)?,
    };
    if sib != o.sib_count {
        return Err(Error::Opening);
    }
    Ok(blake3::parent_output(&kw, KEYED_HASH, &l, &r).root_hash())
}

/// Bytes `[start, start+len)` of the opened matrix (must lie in opened leaves).
fn strip<'a>(p: &'a [u8], o: &Opening, start: u64, len: usize, out: &mut [u8]) -> Result<(), Error> {
    let mut copied = 0usize;
    let end = start + len as u64;
    for i in 0..o.count {
        let ls = o.leaf_index(p, i) as u64 * LEAF as u64;
        let le = ls + LEAF as u64;
        if ls < end && start < le {
            let cs = start.max(ls);
            let ce = end.min(le);
            let d = o.leaf(p, i);
            out[(cs - start) as usize..(ce - start) as usize].copy_from_slice(&d[(cs - ls) as usize..(ce - ls) as usize]);
            copied += (ce - cs) as usize;
        }
    }
    if copied != len {
        return Err(Error::Opening);
    }
    Ok(())
}

/// Which opening.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Side {
    A,
    Bt,
}

/// Step 2: one opening's root equals its commitment and its strips are int7.
pub fn check_opening(p: &[u8], v: &ProofView, job: &JobParams, side: Side) -> Result<(), Error> {
    let c = &job.config;
    let (o, pat, t, want) = match side {
        Side::A => (&v.a, &c.rows, v.t_rows, &job.hash_a),
        Side::Bt => (&v.bt, &c.cols, v.t_cols, &job.hash_b),
    };
    if opening_root(p, o, &job.job_key)? != *want {
        return Err(Error::Commitment);
    }
    let dlen = c.dlen() as usize;
    let mut buf = vec![0u8; dlen];
    for r in 0..pat.size() {
        let row = (pat.index(r) + t) as u64;
        strip(p, o, row * c.k as u64, dlen, &mut buf)?;
        if buf.iter().any(|&x| !(-64..=64).contains(&(x as i8))) {
            return Err(Error::Int7);
        }
    }
    Ok(())
}

fn bind_root(root: &[u8; 32], dim: u32, salt: &[u8; 32]) -> [u8; 32] {
    let mut msg = [0u8; 64];
    msg[..32].copy_from_slice(root);
    msg[32..36].copy_from_slice(&dim.to_le_bytes());
    blake3::keyed_hash(salt, &msg)
}

/// Pow-bound seeds: `(b_noise_seed, a_noise_seed)`.
pub fn pow_seeds(job: &JobParams, pow_digest: &[u8; 32]) -> ([u8; 32], [u8; 32]) {
    let ha = bind_root(&job.hash_a, job.m, &SEED_SALT_A);
    let hb = bind_root(&job.hash_b, job.n, &SEED_SALT_B);
    let mut d = [0u8; 96];
    d[0..32].copy_from_slice(&job.job_key);
    d[32..64].copy_from_slice(pow_digest);
    d[64..96].copy_from_slice(&hb);
    let b = blake3::hash(&d);
    let mut e = [0u8; 64];
    e[..32].copy_from_slice(&b);
    e[32..].copy_from_slice(&ha);
    (b, blake3::hash(&e))
}

/// `noise::random_hash`: keyed blake3 of a 64-byte message (one compression).
#[inline(always)]
fn random_hash(kw: &Cv, index: u32, seed: &[u8; 32], prepend: usize) -> [u8; 32] {
    let mut msg = [0u8; 64];
    msg[prepend * 4..prepend * 4 + 4].copy_from_slice(&((1 + index) as i32).to_le_bytes());
    msg[32..].copy_from_slice(seed);
    blake3::chunk_output(kw, KEYED_HASH, &msg, 0).root_hash()
}

/// `uniform_rows` for one row: r values in [-32, 31].
fn uniform_row(kw: &Cv, label: &[u8; 32], row: u64, r: usize, out: &mut [i8]) {
    let start = row * r as u64;
    let mut block = u64::MAX;
    let mut h = [0u8; 32];
    for (j, o) in out.iter_mut().enumerate().take(r) {
        let idx = start + j as u64;
        if idx / 32 != block {
            block = idx / 32;
            h = random_hash(kw, block as u32, label, 0);
        }
        *o = (h[(idx % 32) as usize] & 63) as i8 - 32;
    }
}

/// Noised rows `strip + noise` for one side: `rows x dlen` i8 (values fit: [-127, 127]).
fn noised_rows(p: &[u8], o: &Opening, pat: &Pattern, t: u32, k: u32, r: usize, dlen: usize, seed: &[u8; 32], label: &[u8; 32]) -> Result<Vec<i8>, Error> {
    let kw = blake3::key_words(seed);
    let h = pat.size() as usize;
    let mut u = vec![0i8; h * r];
    let mut out = vec![0i8; h * dlen];
    for i in 0..h {
        let row = (pat.index(i as u32) + t) as u64;
        uniform_row(&kw, label, row, r, &mut u[i * r..(i + 1) * r]);
        let dst = &mut out[i * dlen..(i + 1) * dlen];
        // raw strip bytes first
        let dst_u8: &mut [u8] = unsafe { core::slice::from_raw_parts_mut(dst.as_mut_ptr() as *mut u8, dlen) };
        strip(p, o, row * k as u64, dlen, dst_u8)?;
    }
    let mask = (r - 1) as u32;
    for blk in 0..dlen / 8 {
        let hh = random_hash(&kw, blk as u32, label, 1);
        for j in 0..8 {
            let l = blk * 8 + j;
            let x = u32::from_le_bytes([hh[4 * j], hh[4 * j + 1], hh[4 * j + 2], hh[4 * j + 3]]);
            let pi = (x & mask) as usize;
            let qi = (pi as u32 ^ (1 + (((r - 1) as u64 * x as u64) >> 32) as u32)) as usize;
            for row in 0..h {
                let ur = &u[row * r..(row + 1) * r];
                let e = (ur[pi] as i32 - ur[qi] as i32) as i8;
                let cell = &mut out[row * dlen + l];
                *cell = (*cell as i32 + e as i32) as i8;
            }
        }
    }
    Ok(out)
}

/// Step 3a: one side's noised strips, `rows x dlen` i8 (A: h rows, B^T: w rows).
pub fn noised_side(p: &[u8], v: &ProofView, job: &JobParams, pow_digest: &[u8; 32], side: Side) -> Result<Vec<i8>, Error> {
    let c = &job.config;
    let (k, r, dlen) = (c.k, c.rank as usize, c.dlen() as usize);
    let (b_seed, a_seed) = pow_seeds(job, pow_digest);
    match side {
        Side::A => noised_rows(p, &v.a, &c.rows, v.t_rows, k, r, dlen, &a_seed, &LABEL_A),
        Side::Bt => noised_rows(p, &v.bt, &c.cols, v.t_cols, k, r, dlen, &b_seed, &LABEL_B),
    }
}

/// Step 3b: tile jackpot over noised strips, jackpot hash and the difficulty check.
pub fn jackpot_noised(a: &[i8], bt: &[i8], job: &JobParams, pow_digest: &[u8; 32], target_bits: u32) -> Result<[u8; 32], Error> {
    let c = &job.config;
    let (r, dlen) = (c.rank as usize, c.dlen() as usize);
    let (_, a_seed) = pow_seeds(job, pow_digest);
    let (h, w) = (c.h() as usize, c.w() as usize);
    if a.len() < h * dlen || bt.len() < w * dlen {
        return Err(Error::Dims);
    }
    let mut acc = [0i32; 256];
    let mut msg = [0u32; JACKPOT_SIZE];
    let mut ll = r;
    while ll <= dlen {
        for u in 0..h {
            let ar = &a[u * dlen + ll - r..u * dlen + ll];
            for vv in 0..w {
                let br = &bt[vv * dlen + ll - r..vv * dlen + ll];
                let mut s = 0i32;
                for l in 0..r {
                    s = s.wrapping_add(ar[l] as i32 * br[l] as i32);
                }
                acc[u * w + vv] = acc[u * w + vv].wrapping_add(s);
            }
        }
        let x = acc[..h * w].iter().fold(0u32, |a, &x| a ^ x as u32);
        let tid = (ll / r - 1) % JACKPOT_SIZE;
        msg[tid] = msg[tid].rotate_left(13) ^ x;
        ll += r;
    }
    let mut b = [0u8; 64];
    for (i, wd) in msg.iter().enumerate() {
        b[i * 4..i * 4 + 4].copy_from_slice(&wd.to_le_bytes());
    }
    let jh = blake3::keyed_hash(&a_seed, &b);
    let bound = difficulty_bound(target_bits, c.penalized_factor());
    if !meets_target(&jh, &bound) {
        return Err(Error::Target);
    }
    Ok(jh)
}

/// Step 3 in one call: seeds, noise, jackpot and the difficulty check.
pub fn check_jackpot(p: &[u8], v: &ProofView, job: &JobParams, pow_digest: &[u8; 32], target_bits: u32) -> Result<[u8; 32], Error> {
    let a = noised_side(p, v, job, pow_digest, Side::A)?;
    let bt = noised_side(p, v, job, pow_digest, Side::Bt)?;
    jackpot_noised(&a, &bt, job, pow_digest, target_bits)
}

/// All steps in one call (native use and tests).
pub fn verify_block_proof(p: &[u8], job: &JobParams, pow_digest: &[u8; 32], target_bits: u32, nonce: u64) -> Result<[u8; 32], Error> {
    let v = crate::proof::parse(p)?;
    check_structure(p, &v, job, nonce)?;
    check_opening(p, &v, job, Side::A)?;
    check_opening(p, &v, job, Side::Bt)?;
    check_jackpot(p, &v, job, pow_digest, target_bits)
}
