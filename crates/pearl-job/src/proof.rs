//! Plain (non-ZK) share proof and its byte format. See PROOF-FORMAT.md.

use anyhow::{bail, ensure, Result};

use crate::merkle::{Digest, MerkleProof, CHUNK_LEN};

pub const MAGIC: [u8; 4] = *b"PSJ1";
pub const VERSION: u8 = 1;
/// Seed mode byte: 2 = miner-bound (the only mode the chain accepts).
pub const SEED_MODE_MINER_BOUND: u8 = 2;
/// Seed mode byte: 3 = pow-bound (v2 block proofs, `pow::BlockProof`).
pub const SEED_MODE_POW_BOUND: u8 = 3;
/// Upper bound on a serialised proof, so a parser never allocates unboundedly.
pub const MAX_PROOF_LEN: usize = 4 << 20;

/// A share: the winning tile's rows of A and rows of B^T, opened against the job's
/// commitments. Row indices are not carried: they are `pattern + t_rows` from the
/// job's config. Merkle roots are not carried: the verifier recomputes them and
/// compares them with the job's `hash_a` / `hash_b`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlainProof {
    pub m: u32,
    pub n: u32,
    pub k: u32,
    pub rank: u16,
    pub nonce: u64,
    pub t_rows: u32,
    pub t_cols: u32,
    pub a: MerkleProof,
    pub bt: MerkleProof,
}

fn put_opening(out: &mut Vec<u8>, p: &MerkleProof) {
    out.extend_from_slice(&(p.total_leaves as u32).to_le_bytes());
    out.extend_from_slice(&(p.leaf_indices.len() as u16).to_le_bytes());
    for &i in &p.leaf_indices {
        out.extend_from_slice(&(i as u32).to_le_bytes());
    }
    for d in &p.leaf_data {
        out.extend_from_slice(d);
    }
    out.extend_from_slice(&(p.siblings.len() as u16).to_le_bytes());
    for s in &p.siblings {
        out.extend_from_slice(s);
    }
}

struct Reader<'a> {
    b: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    fn take(&mut self, n: usize) -> Result<&'a [u8]> {
        ensure!(self.pos + n <= self.b.len(), "proof truncated at byte {}", self.pos);
        let s = &self.b[self.pos..self.pos + n];
        self.pos += n;
        Ok(s)
    }
    fn u16(&mut self) -> Result<u16> {
        Ok(u16::from_le_bytes(self.take(2)?.try_into().unwrap()))
    }
    fn u32(&mut self) -> Result<u32> {
        Ok(u32::from_le_bytes(self.take(4)?.try_into().unwrap()))
    }
    fn u64(&mut self) -> Result<u64> {
        Ok(u64::from_le_bytes(self.take(8)?.try_into().unwrap()))
    }
    fn opening(&mut self) -> Result<MerkleProof> {
        let total_leaves = self.u32()? as usize;
        let lc = self.u16()? as usize;
        let mut leaf_indices = Vec::with_capacity(lc);
        for _ in 0..lc {
            leaf_indices.push(self.u32()? as usize);
        }
        let mut leaf_data = Vec::with_capacity(lc);
        for _ in 0..lc {
            leaf_data.push(<[u8; CHUNK_LEN]>::try_from(self.take(CHUNK_LEN)?).unwrap());
        }
        let sc = self.u16()? as usize;
        let mut siblings = Vec::with_capacity(sc);
        for _ in 0..sc {
            siblings.push(<Digest>::try_from(self.take(32)?).unwrap());
        }
        Ok(MerkleProof { leaf_indices, leaf_data, total_leaves, siblings })
    }
}

impl PlainProof {
    /// PSJ1 bytes with seed mode 2 (miner-bound share).
    pub fn to_bytes(&self) -> Vec<u8> {
        self.to_bytes_with_mode(SEED_MODE_MINER_BOUND)
    }

    /// PSJ1 bytes with an explicit seed mode byte.
    pub fn to_bytes_with_mode(&self, mode: u8) -> Vec<u8> {
        let mut out = Vec::with_capacity(64 + (self.a.leaf_data.len() + self.bt.leaf_data.len()) * (CHUNK_LEN + 4));
        out.extend_from_slice(&MAGIC);
        out.push(VERSION);
        out.push(mode);
        out.extend_from_slice(&self.m.to_le_bytes());
        out.extend_from_slice(&self.n.to_le_bytes());
        out.extend_from_slice(&self.k.to_le_bytes());
        out.extend_from_slice(&self.rank.to_le_bytes());
        out.extend_from_slice(&self.nonce.to_le_bytes());
        out.extend_from_slice(&self.t_rows.to_le_bytes());
        out.extend_from_slice(&self.t_cols.to_le_bytes());
        put_opening(&mut out, &self.a);
        put_opening(&mut out, &self.bt);
        out
    }

    /// Parse a seed-mode-2 (miner-bound share) proof.
    pub fn from_bytes(b: &[u8]) -> Result<Self> {
        Self::from_bytes_with_mode(b, SEED_MODE_MINER_BOUND)
    }

    /// Parse a proof whose seed mode byte must equal `want_mode`.
    pub fn from_bytes_with_mode(b: &[u8], want_mode: u8) -> Result<Self> {
        ensure!(b.len() <= MAX_PROOF_LEN, "proof too large: {} bytes", b.len());
        let mut r = Reader { b, pos: 0 };
        ensure!(r.take(4)? == MAGIC, "bad proof magic");
        let v = r.take(1)?[0];
        ensure!(v == VERSION, "unsupported proof version {v}");
        let mode = r.take(1)?[0];
        if mode != want_mode {
            bail!("unsupported seed mode {mode}");
        }
        let p = PlainProof {
            m: r.u32()?,
            n: r.u32()?,
            k: r.u32()?,
            rank: r.u16()?,
            nonce: r.u64()?,
            t_rows: r.u32()?,
            t_cols: r.u32()?,
            a: r.opening()?,
            bt: r.opening()?,
        };
        ensure!(r.pos == b.len(), "{} trailing bytes after proof", b.len() - r.pos);
        Ok(p)
    }
}
