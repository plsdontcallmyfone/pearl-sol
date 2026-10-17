//! Zero-copy view of a PSJ1 block proof (crates/pearl-job/PROOF-FORMAT.md sections 3, 7).

use crate::Error;

pub const MAGIC: [u8; 4] = *b"PSJ1";
pub const VERSION: u8 = 1;
pub const SEED_MODE_POW_BOUND: u8 = 3;
pub const MAX_PROOF_LEN: usize = 4 << 20;
pub const LEAF: usize = 1024;

/// One Merkle opening inside the proof bytes.
#[derive(Debug, Clone, Copy)]
pub struct Opening {
    pub total_leaves: u32,
    pub count: usize,
    /// Offset of the u32 leaf index array.
    pub idx_off: usize,
    /// Offset of the leaf data (count x 1024).
    pub data_off: usize,
    pub sib_count: usize,
    pub sib_off: usize,
}

impl Opening {
    pub fn leaf_index(&self, p: &[u8], i: usize) -> u32 {
        let o = self.idx_off + 4 * i;
        u32::from_le_bytes(p[o..o + 4].try_into().unwrap())
    }
    pub fn leaf<'a>(&self, p: &'a [u8], i: usize) -> &'a [u8] {
        let o = self.data_off + LEAF * i;
        &p[o..o + LEAF]
    }
    pub fn sibling(&self, p: &[u8], i: usize) -> [u8; 32] {
        let o = self.sib_off + 32 * i;
        p[o..o + 32].try_into().unwrap()
    }
}

#[derive(Debug, Clone, Copy)]
pub struct ProofView {
    pub m: u32,
    pub n: u32,
    pub k: u32,
    pub rank: u16,
    pub nonce: u64,
    pub t_rows: u32,
    pub t_cols: u32,
    pub a: Opening,
    pub bt: Opening,
}

struct Rd<'a> {
    b: &'a [u8],
    pos: usize,
}

impl Rd<'_> {
    fn skip(&mut self, n: usize) -> Result<usize, Error> {
        let at = self.pos;
        let end = at.checked_add(n).ok_or(Error::ProofFormat)?;
        if end > self.b.len() {
            return Err(Error::ProofFormat);
        }
        self.pos = end;
        Ok(at)
    }
    fn u16(&mut self) -> Result<u16, Error> {
        let o = self.skip(2)?;
        Ok(u16::from_le_bytes([self.b[o], self.b[o + 1]]))
    }
    fn u32(&mut self) -> Result<u32, Error> {
        let o = self.skip(4)?;
        Ok(u32::from_le_bytes(self.b[o..o + 4].try_into().unwrap()))
    }
    fn u64(&mut self) -> Result<u64, Error> {
        let o = self.skip(8)?;
        Ok(u64::from_le_bytes(self.b[o..o + 8].try_into().unwrap()))
    }
    fn opening(&mut self) -> Result<Opening, Error> {
        let total_leaves = self.u32()?;
        let count = self.u16()? as usize;
        let idx_off = self.skip(4 * count)?;
        let data_off = self.skip(LEAF * count)?;
        let sib_count = self.u16()? as usize;
        let sib_off = self.skip(32 * sib_count)?;
        Ok(Opening { total_leaves, count, idx_off, data_off, sib_count, sib_off })
    }
}

/// Parse a seed-mode-3 block proof. Rejects bad magic / version / mode, truncation,
/// trailing bytes and oversize input (`proof::PlainProof::from_bytes_with_mode`).
pub fn parse(b: &[u8]) -> Result<ProofView, Error> {
    if b.len() > MAX_PROOF_LEN {
        return Err(Error::ProofFormat);
    }
    let mut r = Rd { b, pos: 0 };
    let o = r.skip(6)?;
    if b[o..o + 4] != MAGIC || b[o + 4] != VERSION || b[o + 5] != SEED_MODE_POW_BOUND {
        return Err(Error::ProofFormat);
    }
    let v = ProofView {
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
    if r.pos != b.len() {
        return Err(Error::ProofFormat);
    }
    Ok(v)
}
