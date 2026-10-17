//! Block = header + proof bytes + transactions.
//!
//! ```text
//! header [256] || proof_len u32 || proof || tx_count u32 || (tx_len u32 || tx)*
//! ```

use crate::codec::{R, W};
use crate::{blake3_hash, merkle, DecodeError, Hash, Header, Tx};

pub const MAX_PROOF_LEN: usize = 4 << 20;
pub const MAX_BLOCK_TXS: usize = 10_000;
pub const MAX_BLOCK_SIZE: usize = 16 << 20;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Block {
    pub header: Header,
    pub proof: Vec<u8>,
    pub txs: Vec<Tx>,
}

impl Block {
    pub fn hash(&self) -> Hash {
        self.header.hash()
    }

    pub fn encode(&self) -> Vec<u8> {
        let mut w = W::new();
        w.bytes(&self.header.encode()).u32(self.proof.len() as u32).bytes(&self.proof);
        w.bytes(&encode_txs(&self.txs));
        w.into_inner()
    }

    pub fn decode(b: &[u8]) -> Result<Self, DecodeError> {
        if b.len() > MAX_BLOCK_SIZE {
            return Err(DecodeError::TooLarge("block"));
        }
        let mut r = R::new(b);
        let header = Header::decode(r.take(Header::SIZE)?)?;
        let plen = r.u32()? as usize;
        if plen > MAX_PROOF_LEN {
            return Err(DecodeError::TooLarge("proof"));
        }
        let proof = r.take(plen)?.to_vec();
        let txs = read_txs(&mut r)?;
        r.finish()?;
        Ok(Self { header, proof, txs })
    }

    pub fn proof_hash(&self) -> Hash {
        proof_hash(&self.proof)
    }

    pub fn tx_root(&self) -> Hash {
        tx_root(&self.txs)
    }

    /// Context-free checks: proof_hash and tx_root match the header.
    pub fn check_commitments(&self) -> Result<(), &'static str> {
        if self.header.proof_hash != self.proof_hash() {
            return Err("proof_hash mismatch");
        }
        if self.header.tx_root != self.tx_root() {
            return Err("tx_root mismatch");
        }
        Ok(())
    }
}

/// Header plus proof: the unit headers-first sync moves and checks before bodies.
pub fn encode_header_with_proof(h: &Header, proof: &[u8]) -> Vec<u8> {
    let mut w = W::new();
    w.bytes(&h.encode()).bytes(proof);
    w.into_inner()
}

pub fn decode_header_with_proof(b: &[u8]) -> Result<(Header, Vec<u8>), DecodeError> {
    if b.len() < Header::SIZE {
        return Err(DecodeError::Truncated(b.len()));
    }
    if b.len() - Header::SIZE > MAX_PROOF_LEN {
        return Err(DecodeError::TooLarge("proof"));
    }
    Ok((Header::decode(&b[..Header::SIZE])?, b[Header::SIZE..].to_vec()))
}

pub fn encode_txs(txs: &[Tx]) -> Vec<u8> {
    let mut w = W::new();
    w.u32(txs.len() as u32);
    for t in txs {
        let e = t.encode();
        w.u32(e.len() as u32).bytes(&e);
    }
    w.into_inner()
}

fn read_txs(r: &mut R<'_>) -> Result<Vec<Tx>, DecodeError> {
    let n = r.u32()? as usize;
    if n > MAX_BLOCK_TXS {
        return Err(DecodeError::TooLarge("tx count"));
    }
    let mut txs = Vec::with_capacity(n);
    for _ in 0..n {
        let len = r.u32()? as usize;
        txs.push(Tx::decode(r.take(len)?)?);
    }
    Ok(txs)
}

pub fn decode_txs(b: &[u8]) -> Result<Vec<Tx>, DecodeError> {
    let mut r = R::new(b);
    let txs = read_txs(&mut r)?;
    r.finish()?;
    Ok(txs)
}

pub fn proof_hash(proof: &[u8]) -> Hash {
    blake3_hash(&[proof])
}

pub fn tx_root(txs: &[Tx]) -> Hash {
    let hs: Vec<Hash> = txs.iter().map(Tx::hash).collect();
    merkle::root(&hs)
}
