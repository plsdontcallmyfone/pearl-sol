//! Single-element inclusion proofs for the withdrawal log (`withdraw_root`): a
//! Commonware 2026.9.0 QMDB `keyless` database, i.e. an MMR over SHA-256 with the
//! qmdb root bagging (`Bagging::BackwardFold`). Ported from commonware-storage
//! `merkle/{hasher,proof}.rs` (`Proof::verify_element_inclusion`, `Blueprint`).
//!
//! - leaf digest   = sha256(pos u64 BE || element)
//! - node digest   = sha256(pos u64 BE || left || right)
//! - fold(acc, p)  = sha256(acc || p)
//! - root          = sha256(leaves u64 BE || [inactive u64 BE if nonzero] || folded peaks)
//! - proof codec   = varint leaves || varint inactive_peaks || varint count || digests
//! - a withdrawal leaf element = keyless `Operation::Append(Vec<u8>)` encoding:
//!   `0x01 || varint(len) || entry`.

use crate::Error;

pub type Sha = fn(&[&[u8]]) -> [u8; 32];

pub fn read_varint(b: &[u8], pos: &mut usize) -> Result<u64, Error> {
    let mut result: u64 = 0;
    let mut shift = 0u32;
    loop {
        let byte = *b.get(*pos).ok_or(Error::MmrFormat)?;
        *pos += 1;
        if shift == 63 && byte > 1 {
            return Err(Error::MmrFormat);
        }
        result |= ((byte & 0x7f) as u64) << shift;
        if byte & 0x80 == 0 {
            if byte == 0 && shift > 0 {
                return Err(Error::MmrFormat); // non-canonical
            }
            return Ok(result);
        }
        shift += 7;
        if shift > 63 {
            return Err(Error::MmrFormat);
        }
    }
}

pub fn write_varint(mut v: u64, out: &mut [u8; 10]) -> usize {
    let mut n = 0;
    loop {
        let b = (v & 0x7f) as u8;
        v >>= 7;
        if v == 0 {
            out[n] = b;
            return n + 1;
        }
        out[n] = b | 0x80;
        n += 1;
    }
}

/// Parsed proof: `(leaves, inactive_peaks, digests)` with digests borrowed.
pub struct MmrProof<'a> {
    pub leaves: u64,
    pub inactive: u64,
    pub digests: &'a [u8],
}

impl MmrProof<'_> {
    pub fn count(&self) -> usize {
        self.digests.len() / 32
    }
    pub fn digest(&self, i: usize) -> [u8; 32] {
        self.digests[32 * i..32 * i + 32].try_into().unwrap()
    }
}

pub fn parse(b: &[u8]) -> Result<MmrProof<'_>, Error> {
    let mut pos = 0;
    let leaves = read_varint(b, &mut pos)?;
    let inactive = read_varint(b, &mut pos)?;
    let n = read_varint(b, &mut pos)? as usize;
    if n > 128 || b.len() != pos + 32 * n {
        return Err(Error::MmrFormat);
    }
    Ok(MmrProof { leaves, inactive, digests: &b[pos..] })
}

fn leaf_pos(loc: u64) -> u64 {
    2 * loc - loc.count_ones() as u64
}

struct Cursor<'a, 'b> {
    p: &'a MmrProof<'b>,
    next: usize,
    end: usize,
}

impl Cursor<'_, '_> {
    fn take(&mut self) -> Result<[u8; 32], Error> {
        if self.next >= self.end {
            return Err(Error::MmrProof);
        }
        let d = self.p.digest(self.next);
        self.next += 1;
        Ok(d)
    }
}

/// DFS reconstruction of a peak containing `loc` (left-first, siblings in visit order).
fn reconstruct(sha: Sha, pos: u64, height: u32, leaf_start: u64, loc: u64, element: &[u8], sib: &mut Cursor) -> Result<[u8; 32], Error> {
    let leaf_end = leaf_start + (1u64 << height);
    if loc < leaf_start || loc >= leaf_end {
        return sib.take();
    }
    if height == 0 {
        return Ok(sha(&[&pos.to_be_bytes(), element]));
    }
    let left = pos - (1u64 << height);
    let right = pos - 1;
    let mid = leaf_start + (1u64 << (height - 1));
    let l = reconstruct(sha, left, height - 1, leaf_start, loc, element, sib)?;
    let r = reconstruct(sha, right, height - 1, mid, loc, element, sib)?;
    Ok(sha(&[&pos.to_be_bytes(), &l, &r]))
}

/// True iff `proof` shows `element` at location `loc` under `root`.
pub fn verify_inclusion(sha: Sha, proof_bytes: &[u8], element: &[u8], loc: u64, root: &[u8; 32]) -> Result<(), Error> {
    let p = parse(proof_bytes)?;
    if p.leaves == 0 || loc >= p.leaves || p.leaves >= 1u64 << 40 {
        return Err(Error::MmrProof);
    }
    let inactive = p.inactive as usize;
    // Peaks, left to right: one per set bit of `leaves`, highest first.
    let mut peaks: [(u64, u32, u64); 40] = [(0, 0, 0); 40];
    let mut np = 0usize;
    let mut acc_nodes = 0u64;
    let mut leaf_cursor = 0u64;
    for h in (0..40u32).rev() {
        if p.leaves & (1u64 << h) != 0 {
            let pos = acc_nodes + (1u64 << (h + 1)) - 2;
            peaks[np] = (pos, h, leaf_cursor);
            np += 1;
            acc_nodes += (1u64 << (h + 1)) - 1;
            leaf_cursor += 1u64 << h;
        }
    }
    if inactive > np {
        return Err(Error::MmrProof);
    }
    // Classify (BackwardFold).
    let mut fold_prefix = 0usize;
    let mut prefix_active = 0usize;
    let mut after = 0usize;
    let mut suffix = 0usize;
    let mut range_idx = usize::MAX;
    for (i, &(_, h, ls)) in peaks[..np].iter().enumerate() {
        let le = ls + (1u64 << h);
        if le <= loc {
            if i < inactive {
                fold_prefix += 1;
            } else {
                prefix_active += 1;
            }
        } else if ls > loc {
            if i >= inactive {
                suffix += 1;
            } else {
                after += 1;
            }
        } else {
            range_idx = i;
        }
    }
    if range_idx == usize::MAX {
        return Err(Error::MmrProof);
    }
    let fold_count = usize::from(fold_prefix > 0);
    let suffix_count = usize::from(suffix > 0);
    let head = fold_count + prefix_active + after + suffix_count;
    if p.count() < head {
        return Err(Error::MmrProof);
    }
    let (pos, h, ls) = peaks[range_idx];
    let mut sib = Cursor { p: &p, next: head, end: p.count() };
    let range_digest = reconstruct(sha, pos, h, ls, loc, element, &mut sib)?;
    if sib.next != p.count() {
        return Err(Error::MmrProof);
    }
    // Peak digest list in reconstruction order.
    let mut list: [[u8; 32]; 42] = [[0; 32]; 42];
    let mut n = 0usize;
    let mut take = 0usize;
    for _ in 0..fold_count + prefix_active {
        list[n] = p.digest(take);
        n += 1;
        take += 1;
    }
    list[n] = range_digest;
    n += 1;
    for _ in 0..after + suffix_count {
        list[n] = p.digest(take);
        n += 1;
        take += 1;
    }
    let to_fold = if fold_prefix == 0 { inactive } else { inactive.saturating_sub(fold_prefix) + 1 };
    let mut acc = list[0];
    let mut i = 1usize;
    for _ in 0..to_fold.saturating_sub(1) {
        if i >= n {
            return Err(Error::MmrProof);
        }
        acc = sha(&[&acc, &list[i]]);
        i += 1;
    }
    // Backward fold of [acc, list[i..n]].
    let folded = if i >= n {
        acc
    } else {
        let mut b = list[n - 1];
        let mut j = n - 1;
        while j > i {
            j -= 1;
            b = sha(&[&list[j], &b]);
        }
        sha(&[&acc, &b])
    };
    let got = if inactive == 0 {
        sha(&[&p.leaves.to_be_bytes(), &folded])
    } else {
        sha(&[&p.leaves.to_be_bytes(), &(inactive as u64).to_be_bytes(), &folded])
    };
    if got != *root {
        return Err(Error::MmrRoot);
    }
    Ok(())
}

/// Leaf element of a keyless `Append(entry)` operation.
pub fn append_element(entry: &[u8], out: &mut [u8]) -> usize {
    let mut v = [0u8; 10];
    let n = write_varint(entry.len() as u64, &mut v);
    out[0] = 0x01;
    out[1..1 + n].copy_from_slice(&v[..n]);
    out[1 + n..1 + n + entry.len()].copy_from_slice(entry);
    1 + n + entry.len()
}
