//! Final-header accumulator (Agent Credit request R5): a SHA-256 Merkle mountain range
//! over the hashes of the agent chain's final headers, in height order, kept by the
//! light client so a header stays provable after `close_header` removed its account.
//!
//! ```text
//! leaf(block_hash) = sha256(0x00 || block_hash)
//! node(l, r)       = sha256(0x01 || l || r)
//! root(n, peaks)   = sha256(0x02 || n u64 LE || peak* )   peaks left to right
//! ```
//! Leaf `i` is the header at `start_height + i`. With `n` leaves the peaks are the
//! perfect subtrees given by the set bits of `n`, largest (leftmost) first; the light
//! client stores the peak of height `b` in slot `b`.
//!
//! Proof of leaf `i` against `(n, root)`: the `b` siblings inside its peak's subtree
//! (bottom-up, 32 bytes each, sides given by the bits of `i - subtree start`) followed
//! by every OTHER peak left to right (32 bytes each). Its length is fixed by `n` and `i`.

extern crate alloc;
use alloc::vec::Vec;

use crate::Error;

pub type Sha = fn(&[&[u8]]) -> [u8; 32];

pub const MAX_PEAKS: usize = 64;

pub fn leaf(sha: Sha, block_hash: &[u8; 32]) -> [u8; 32] {
    sha(&[&[0u8], block_hash])
}

pub fn node(sha: Sha, l: &[u8; 32], r: &[u8; 32]) -> [u8; 32] {
    sha(&[&[1u8], l, r])
}

/// Root over the peaks of an `n`-leaf range given left to right.
pub fn root_of_peaks(sha: Sha, n: u64, peaks_left_to_right: &[[u8; 32]]) -> [u8; 32] {
    let mut buf = [0u8; 8 + 32 * MAX_PEAKS];
    buf[..8].copy_from_slice(&n.to_le_bytes());
    for (i, p) in peaks_left_to_right.iter().enumerate() {
        buf[8 + 32 * i..40 + 32 * i].copy_from_slice(p);
    }
    sha(&[&[2u8], &buf[..8 + 32 * peaks_left_to_right.len()]])
}

/// Root from the light client's slot-indexed peaks (`slots[b]` = peak of height `b`).
pub fn root_of_slots(sha: Sha, n: u64, slots: &[[u8; 32]; MAX_PEAKS]) -> [u8; 32] {
    let mut v: Vec<[u8; 32]> = Vec::new();
    for b in (0..64).rev() {
        if n >> b & 1 == 1 {
            v.push(slots[b]);
        }
    }
    root_of_peaks(sha, n, &v)
}

/// Append a leaf to slot-indexed peaks holding `n` leaves (n becomes n + 1).
pub fn append(sha: Sha, n: u64, slots: &mut [[u8; 32]; MAX_PEAKS], block_hash: &[u8; 32]) {
    let mut carry = leaf(sha, block_hash);
    let mut b = 0;
    while n >> b & 1 == 1 {
        carry = node(sha, &slots[b], &carry);
        b += 1;
    }
    slots[b] = carry;
}

/// `(subtree start, subtree height, peak position left to right, peak count)` of leaf `i`.
fn locate(n: u64, i: u64) -> Option<(u64, u32, usize, usize)> {
    if i >= n {
        return None;
    }
    let mut off = 0u64;
    let mut pos = 0usize;
    let mut found = None;
    for b in (0..64u32).rev() {
        if n >> b & 1 == 1 {
            let size = 1u64 << b;
            if found.is_none() && i < off + size {
                found = Some((off, b, pos));
            }
            off += size;
            pos += 1;
        }
    }
    found.map(|(o, b, p)| (o, b, p, pos))
}

/// Expected proof length for leaf `i` of `n`.
pub fn proof_len(n: u64, i: u64) -> Option<usize> {
    locate(n, i).map(|(_, b, _, peaks)| 32 * (b as usize + peaks - 1))
}

/// Check that `block_hash` is leaf `i` of the `n`-leaf accumulator with `root`.
pub fn verify(sha: Sha, n: u64, i: u64, block_hash: &[u8; 32], proof: &[u8], root: &[u8; 32]) -> Result<(), Error> {
    let (start, b, pos, peaks) = locate(n, i).ok_or(Error::AccProof)?;
    if peaks > MAX_PEAKS || proof.len() != 32 * (b as usize + peaks - 1) {
        return Err(Error::AccProof);
    }
    let mut h = leaf(sha, block_hash);
    let rel = i - start;
    for lvl in 0..b as usize {
        let sib: [u8; 32] = proof[32 * lvl..32 * lvl + 32].try_into().unwrap();
        h = if rel >> lvl & 1 == 1 { node(sha, &sib, &h) } else { node(sha, &h, &sib) };
    }
    let rest = &proof[32 * b as usize..];
    let mut v: Vec<[u8; 32]> = Vec::with_capacity(peaks);
    let mut k = 0;
    for p in 0..peaks {
        if p == pos {
            v.push(h);
        } else {
            v.push(rest[32 * k..32 * k + 32].try_into().unwrap());
            k += 1;
        }
    }
    if root_of_peaks(sha, n, &v) != *root {
        return Err(Error::AccProof);
    }
    Ok(())
}

/// Off-chain helpers (relayers, tests): root and proofs from the full leaf list.
pub fn root_of_hashes(sha: Sha, block_hashes: &[[u8; 32]]) -> [u8; 32] {
    let mut slots = [[0u8; 32]; MAX_PEAKS];
    for (n, h) in block_hashes.iter().enumerate() {
        append(sha, n as u64, &mut slots, h);
    }
    root_of_slots(sha, block_hashes.len() as u64, &slots)
}

fn subtree_root(sha: Sha, hashes: &[[u8; 32]]) -> [u8; 32] {
    if hashes.len() == 1 {
        return leaf(sha, &hashes[0]);
    }
    let mid = hashes.len() / 2;
    node(sha, &subtree_root(sha, &hashes[..mid]), &subtree_root(sha, &hashes[mid..]))
}

/// Proof of leaf `i` against the accumulator over `block_hashes` (n = its length).
pub fn prove(sha: Sha, block_hashes: &[[u8; 32]], i: u64) -> Option<Vec<u8>> {
    let n = block_hashes.len() as u64;
    let (start, b, pos, peaks) = locate(n, i)?;
    let mut out = Vec::new();
    let sub = &block_hashes[start as usize..(start + (1u64 << b)) as usize];
    let rel = (i - start) as usize;
    for lvl in 0..b {
        let w = 1usize << lvl;
        let sib_start = ((rel >> lvl) ^ 1) * w;
        out.extend_from_slice(&subtree_root(sha, &sub[sib_start..sib_start + w]));
    }
    let mut off = 0u64;
    let mut p = 0usize;
    for bb in (0..64u32).rev() {
        if n >> bb & 1 == 1 {
            let size = 1u64 << bb;
            if p != pos {
                out.extend_from_slice(&subtree_root(sha, &block_hashes[off as usize..(off + size) as usize]));
            }
            off += size;
            p += 1;
        }
    }
    debug_assert_eq!(p, peaks);
    Some(out)
}
