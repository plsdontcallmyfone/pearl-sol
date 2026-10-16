//! Keyed BLAKE3 Merkle tree over 1024-byte chunks, with multi-leaf proofs.
//!
//! The tree is BLAKE3's own chunk tree, so `root == blake3_keyed(data, key)`:
//! anyone with a stock BLAKE3 library can compute a commitment.
//! Ported from Pearl `pearl-blake3/src/{hasher,merkle}.rs`, ISC. Single-threaded on
//! purpose (deterministic, and close to what an SBF port will do).

use std::collections::{BTreeMap, BTreeSet};

use anyhow::{ensure, Result};
use blake3::hazmat::{merge_subtrees_non_root, merge_subtrees_root, HasherExt, Mode};

pub const CHUNK_LEN: usize = 1024;
pub type Digest = [u8; 32];

pub fn padded_chunk_len(raw_len: usize) -> usize {
    raw_len.div_ceil(CHUNK_LEN) * CHUNK_LEN
}

pub fn pad_to_chunk_boundary(data: &[u8]) -> Vec<u8> {
    let mut v = data.to_vec();
    v.resize(padded_chunk_len(data.len()), 0);
    v
}

pub fn blake3_digest(data: &[u8], key: Option<&Digest>) -> Digest {
    match key {
        Some(k) => *blake3::keyed_hash(k, data).as_bytes(),
        None => *blake3::hash(data).as_bytes(),
    }
}

/// Non-root chaining value of chunk `index` (data at most 1024 bytes).
pub fn chunk_cv(key: &Digest, data: &[u8], index: u64) -> Digest {
    let mut h = blake3::Hasher::new_keyed(key);
    h.set_input_offset(index * CHUNK_LEN as u64);
    h.update(data);
    h.finalize_non_root()
}

pub fn parent_cv(key: &Digest, l: &Digest, r: &Digest) -> Digest {
    merge_subtrees_non_root(l, r, Mode::KeyedHash(key))
}

pub fn root_cv(key: &Digest, l: &Digest, r: &Digest) -> Digest {
    *merge_subtrees_root(l, r, Mode::KeyedHash(key)).as_bytes()
}

pub struct MerkleTree {
    key: Digest,
    layers: Vec<Vec<Digest>>,
    data: Vec<u8>,
}

impl MerkleTree {
    /// `data` should already be padded to a chunk boundary.
    pub fn new(data: &[u8], key: Digest) -> Self {
        if data.len() <= CHUNK_LEN {
            let root = blake3_digest(data, Some(&key));
            return Self { key, layers: vec![vec![root]], data: data.to_vec() };
        }
        let leaves: Vec<Digest> =
            data.chunks(CHUNK_LEN).enumerate().map(|(i, c)| chunk_cv(&key, c, i as u64)).collect();
        let mut layers = vec![leaves];
        while layers.last().unwrap().len() > 2 {
            let prev = layers.last().unwrap();
            let next = prev
                .chunks(2)
                .map(|p| if p.len() == 2 { parent_cv(&key, &p[0], &p[1]) } else { p[0] })
                .collect();
            layers.push(next);
        }
        let last = layers.last().unwrap();
        let root = root_cv(&key, &last[0], &last[1]);
        layers.push(vec![root]);
        Self { key, layers, data: data.to_vec() }
    }

    pub fn root(&self) -> Digest {
        self.layers.last().unwrap()[0]
    }

    pub fn num_leaves(&self) -> usize {
        self.data.len().div_ceil(CHUNK_LEN).max(1)
    }

    pub fn key(&self) -> Digest {
        self.key
    }

    pub fn multileaf_proof(&self, leaf_indices: &[usize]) -> MerkleProof {
        let unique: BTreeSet<usize> = leaf_indices.iter().copied().collect();
        let total = self.num_leaves();
        assert!(!unique.is_empty() && *unique.last().unwrap() < total, "leaf index out of bounds");
        let sorted: Vec<usize> = unique.iter().copied().collect();
        let leaf_data = sorted
            .iter()
            .map(|&i| {
                let s = i * CHUNK_LEN;
                let e = (s + CHUNK_LEN).min(self.data.len());
                let mut c = [0u8; CHUNK_LEN];
                c[..e - s].copy_from_slice(&self.data[s..e]);
                c
            })
            .collect();
        let mut siblings = Vec::new();
        let mut cur = unique;
        let mut level_len = total;
        let mut level = 0;
        while level_len > 1 {
            let nodes = &self.layers[level];
            for &i in &cur {
                if i % 2 == 1 {
                    if !cur.contains(&(i - 1)) {
                        siblings.push(nodes[i - 1]);
                    }
                } else if !cur.contains(&(i + 1)) && i + 1 < level_len {
                    siblings.push(nodes[i + 1]);
                }
            }
            cur = cur.iter().map(|&i| i / 2).collect();
            level_len = level_len.div_ceil(2);
            level += 1;
        }
        MerkleProof { leaf_indices: sorted, leaf_data, total_leaves: total, siblings }
    }

    /// Leaves covering the given rows of a row-major `(rows, cols)` byte matrix.
    pub fn leaf_indices_for_rows(rows: &[usize], cols: usize) -> Vec<usize> {
        let mut set = BTreeSet::new();
        for &row in rows {
            let first = (row * cols) / CHUNK_LEN;
            let last = ((row + 1) * cols - 1) / CHUNK_LEN;
            set.extend(first..=last);
        }
        set.into_iter().collect()
    }
}

/// Multi-leaf proof. The root is not carried: the verifier recomputes it and
/// compares it with the job's on-chain commitment.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MerkleProof {
    pub leaf_indices: Vec<usize>,
    pub leaf_data: Vec<[u8; CHUNK_LEN]>,
    pub total_leaves: usize,
    pub siblings: Vec<Digest>,
}

impl MerkleProof {
    pub fn compute_root(&self, key: &Digest) -> Option<Digest> {
        if self.leaf_indices.is_empty() || self.leaf_indices.len() != self.leaf_data.len() {
            return None;
        }
        if !self.leaf_indices.windows(2).all(|w| w[0] < w[1]) {
            return None;
        }
        if *self.leaf_indices.last().unwrap() >= self.total_leaves {
            return None;
        }
        if self.total_leaves == 1 {
            return if self.siblings.is_empty() { Some(blake3_digest(&self.leaf_data[0], Some(key))) } else { None };
        }
        let mut cur: BTreeMap<usize, Digest> = self
            .leaf_indices
            .iter()
            .zip(&self.leaf_data)
            .map(|(&i, d)| (i, chunk_cv(key, d, i as u64)))
            .collect();
        let mut level_len = self.total_leaves;
        let mut sib = self.siblings.iter();
        while level_len > 2 {
            let mut next = BTreeMap::new();
            for (&i, &cv) in &cur {
                if i % 2 == 0 {
                    let right = if let Some(&r) = cur.get(&(i + 1)) {
                        Some(r)
                    } else if i + 1 < level_len {
                        Some(*sib.next()?)
                    } else {
                        None
                    };
                    next.insert(i / 2, match right {
                        Some(r) => parent_cv(key, &cv, &r),
                        None => cv,
                    });
                } else if cur.contains_key(&(i - 1)) {
                    continue;
                } else {
                    let left = *sib.next()?;
                    next.insert(i / 2, parent_cv(key, &left, &cv));
                }
            }
            cur = next;
            level_len = level_len.div_ceil(2);
        }
        let l = match cur.get(&0) {
            Some(&v) => v,
            None => *sib.next()?,
        };
        let r = match cur.get(&1) {
            Some(&v) => v,
            None => *sib.next()?,
        };
        if sib.next().is_some() {
            return None;
        }
        Some(root_cv(key, &l, &r))
    }

    pub fn extract_bytes(&self, start: usize, len: usize) -> Result<Vec<u8>> {
        let mut out = vec![0u8; len];
        let end = start + len;
        let mut copied = 0;
        for (&li, d) in self.leaf_indices.iter().zip(&self.leaf_data) {
            let ls = li * CHUNK_LEN;
            let le = ls + CHUNK_LEN;
            if ls < end && start < le {
                let cs = start.max(ls);
                let ce = end.min(le);
                out[cs - start..ce - start].copy_from_slice(&d[cs - ls..ce - ls]);
                copied += ce - cs;
            }
        }
        ensure!(copied == len, "bytes [{start}, {end}) not covered by proof leaves");
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn root_is_stock_keyed_blake3_and_proofs_verify() {
        let key = [7u8; 32];
        for chunks in [1usize, 2, 3, 5, 8, 13] {
            let data: Vec<u8> = (0..chunks * CHUNK_LEN).map(|i| (i * 31 % 251) as u8).collect();
            let t = MerkleTree::new(&data, key);
            assert_eq!(t.root(), blake3_digest(&data, Some(&key)));
            for sel in [vec![0], vec![chunks - 1], (0..chunks).step_by(2).collect::<Vec<_>>()] {
                let p = t.multileaf_proof(&sel);
                assert_eq!(p.compute_root(&key), Some(t.root()), "chunks={chunks} sel={sel:?}");
            }
        }
    }
}
