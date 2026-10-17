//! `tx_root`: binary blake3 Merkle tree over transaction hashes.
//!
//! - leaf = blake3(0x00 || tx_hash)
//! - node = blake3(0x01 || left || right); an odd node at the end of a level is
//!   carried up unchanged
//! - empty tree root = 32 zero bytes

use crate::{blake3_hash, Hash};

pub fn leaf(tx_hash: &Hash) -> Hash {
    blake3_hash(&[&[0u8], tx_hash])
}

pub fn node(l: &Hash, r: &Hash) -> Hash {
    blake3_hash(&[&[1u8], l, r])
}

pub fn root(tx_hashes: &[Hash]) -> Hash {
    if tx_hashes.is_empty() {
        return [0u8; 32];
    }
    let mut level: Vec<Hash> = tx_hashes.iter().map(leaf).collect();
    while level.len() > 1 {
        level = level
            .chunks(2)
            .map(|c| if c.len() == 2 { node(&c[0], &c[1]) } else { c[0] })
            .collect();
    }
    level[0]
}

/// Sibling path for leaf `index` (bottom-up). Each entry is `(sibling, sibling_is_left)`;
/// levels where the node is carried up have no entry.
pub fn proof(tx_hashes: &[Hash], mut index: usize) -> Vec<(Hash, bool)> {
    let mut out = Vec::new();
    let mut level: Vec<Hash> = tx_hashes.iter().map(leaf).collect();
    while level.len() > 1 {
        let sib = index ^ 1;
        if sib < level.len() {
            out.push((level[sib], sib < index));
        }
        level = level
            .chunks(2)
            .map(|c| if c.len() == 2 { node(&c[0], &c[1]) } else { c[0] })
            .collect();
        index /= 2;
    }
    out
}

pub fn verify(tx_hash: &Hash, path: &[(Hash, bool)], root: &Hash) -> bool {
    let mut h = leaf(tx_hash);
    for (sib, is_left) in path {
        h = if *is_left { node(sib, &h) } else { node(&h, sib) };
    }
    h == *root
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn proofs_verify() {
        for n in 1..12usize {
            let hs: Vec<Hash> = (0..n).map(|i| blake3_hash(&[&[i as u8]])).collect();
            let r = root(&hs);
            for i in 0..n {
                assert!(verify(&hs[i], &proof(&hs, i), &r), "n={n} i={i}");
            }
        }
        assert_eq!(root(&[]), [0u8; 32]);
    }
}
