//! Block = header + proof bytes + transactions + coinbase receipt.
//!
//! ```text
//! header [256] || proof_len u32 || proof || tx_count u32 || (tx_len u32 || tx)*
//!   || has_coinbase u8 (0|1) || [coinbase receipt 97 bytes if 1]
//!   || [0x01 || contracts record 89 bytes]   (only in a block with contract txs)
//!   || [0x02 || count u8 || (uncle header [256] || proof_len u32 || proof || uncle record 137)*]
//!                                            (only in a block with uncles, docs/SPEED.md 3.3)
//! ```
//! The contracts record (contracts.rs, from the contracts activation height) is a tx_root
//! leaf placed right before the coinbase receipt; a block without contract txs has none and
//! encodes exactly as before.
//! Every non-genesis block carries its coinbase receipt (coinbase.rs), committed as the
//! LAST leaf of `tx_root`; the genesis block carries none (its tx_root commits to the
//! genesis spec). The receipt travels in the body so `check_commitments` stays
//! context-free; the chain then checks it equals the receipt execution produces.

use crate::codec::{R, W};
use crate::coinbase::{Coinbase, COINBASE_LEN};
use crate::contracts::{ContractsRecord, RECORD_LEN};
use crate::speed::{Uncle, UncleRecord, MAX_UNCLES, UNCLE_RECORD_LEN};
use crate::{blake3_hash, merkle, DecodeError, Hash, Header, Tx};

pub const MAX_PROOF_LEN: usize = 4 << 20;
pub const MAX_BLOCK_TXS: usize = 10_000;
pub const MAX_BLOCK_SIZE: usize = 16 << 20;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Block {
    pub header: Header,
    pub proof: Vec<u8>,
    pub txs: Vec<Tx>,
    /// `None` only for the genesis block.
    pub coinbase: Option<Coinbase>,
    /// Contracts record of a block with contract txs (docs/CONTRACTS.md).
    pub contracts: Option<ContractsRecord>,
    /// Uncles credited by this block (docs/SPEED.md 3.3); empty encodes as before.
    pub uncles: Vec<Uncle>,
}

impl Block {
    pub fn hash(&self) -> Hash {
        self.header.hash()
    }

    pub fn encode(&self) -> Vec<u8> {
        let mut w = W::new();
        w.bytes(&self.header.encode()).u32(self.proof.len() as u32).bytes(&self.proof);
        w.bytes(&encode_txs(&self.txs));
        match &self.coinbase {
            None => {
                w.u8(0);
            }
            Some(c) => {
                w.u8(1).bytes(&c.encode());
            }
        }
        if let Some(c) = &self.contracts {
            w.u8(1).bytes(&c.encode());
        }
        if !self.uncles.is_empty() {
            w.u8(2).u8(self.uncles.len() as u8);
            for u in &self.uncles {
                w.bytes(&u.header.encode()).u32(u.proof.len() as u32).bytes(&u.proof).bytes(&u.record.encode());
            }
        }
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
        let coinbase = match r.u8()? {
            0 => None,
            1 => Some(Coinbase::decode(r.take(COINBASE_LEN)?)?),
            _ => return Err(DecodeError::Invalid("coinbase flag")),
        };
        let mut contracts = None;
        let mut uncles = Vec::new();
        let mut last_tag = 0u8;
        while r.remaining() > 0 {
            let tag = r.u8()?;
            if tag <= last_tag {
                return Err(DecodeError::Invalid("body section order"));
            }
            last_tag = tag;
            match tag {
                1 => contracts = Some(ContractsRecord::decode(r.take(RECORD_LEN)?)?),
                2 => {
                    let n = r.u8()? as usize;
                    if n == 0 || n > MAX_UNCLES {
                        return Err(DecodeError::Invalid("uncle count"));
                    }
                    for _ in 0..n {
                        let header = Header::decode(r.take(Header::SIZE)?)?;
                        let plen = r.u32()? as usize;
                        if plen > MAX_PROOF_LEN {
                            return Err(DecodeError::TooLarge("uncle proof"));
                        }
                        let proof = r.take(plen)?.to_vec();
                        let record = UncleRecord::decode(r.take(UNCLE_RECORD_LEN)?)?;
                        uncles.push(Uncle { header, proof, record });
                    }
                }
                _ => return Err(DecodeError::Invalid("contracts flag")),
            }
        }
        r.finish()?;
        Ok(Self { header, proof, txs, coinbase, contracts, uncles })
    }

    pub fn proof_hash(&self) -> Hash {
        proof_hash(&self.proof)
    }

    pub fn tx_root(&self) -> Hash {
        merkle::root(&self.leaf_items())
    }

    /// tx_root leaf items: tx hashes, the contracts record, the uncle records, the receipt.
    pub fn leaf_items(&self) -> Vec<Hash> {
        let recs: Vec<UncleRecord> = self.uncles.iter().map(|u| u.record).collect();
        leaf_hashes_all(&self.txs, self.contracts.as_ref(), &recs, self.coinbase.as_ref())
    }

    /// tx_root path of leaf `index` (a tx index; [`Block::contracts_index`]; [`Block::coinbase_index`]).
    pub fn leaf_path(&self, index: usize) -> Vec<u8> {
        crate::contracts::path_bytes(&self.leaf_items(), index)
    }

    /// Leaf index of the contracts record (when the block has one).
    pub fn contracts_index(&self) -> usize {
        self.txs.len()
    }

    /// Leaf index of the coinbase receipt (the last leaf).
    pub fn coinbase_index(&self) -> usize {
        self.txs.len() + self.contracts.is_some() as usize + self.uncles.len()
    }

    /// Leaf index of uncle record `i`.
    pub fn uncle_index(&self, i: usize) -> usize {
        self.txs.len() + self.contracts.is_some() as usize + i
    }

    /// Context-free checks: proof_hash and tx_root match the header.
    pub fn check_commitments(&self) -> Result<(), &'static str> {
        if self.header.proof_hash != self.proof_hash() {
            return Err("proof_hash mismatch");
        }
        let Some(cb) = &self.coinbase else { return Err("missing coinbase receipt") };
        if cb.height != self.header.height || cb.miner != self.header.miner || cb.job_id != self.header.job_id {
            return Err("coinbase receipt does not match the header");
        }
        if let Some(c) = &self.contracts {
            if c.height != self.header.height {
                return Err("contracts record does not match the header");
            }
        }
        if self.uncles.len() > MAX_UNCLES {
            return Err("too many uncles");
        }
        for (i, u) in self.uncles.iter().enumerate() {
            u.check(self.header.height)?;
            if self.uncles[..i].iter().any(|v| v.record.uncle_hash == u.record.uncle_hash) {
                return Err("duplicate uncle");
            }
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

/// Root over the txs only (no receipt). Only for pre-R1 data; blocks use [`tx_root_with`].
pub fn tx_root(txs: &[Tx]) -> Hash {
    let hs: Vec<Hash> = txs.iter().map(Tx::hash).collect();
    merkle::root(&hs)
}

/// Leaf hashes of a block's tx_root: every tx hash in order, then the receipt hash.
pub fn leaf_hashes(txs: &[Tx], coinbase: Option<&Coinbase>) -> Vec<Hash> {
    let mut hs: Vec<Hash> = txs.iter().map(Tx::hash).collect();
    if let Some(c) = coinbase {
        hs.push(c.hash());
    }
    hs
}

/// A block's `tx_root`: txs then the coinbase receipt as the last leaf.
pub fn tx_root_with(txs: &[Tx], coinbase: Option<&Coinbase>) -> Hash {
    merkle::root(&leaf_hashes(txs, coinbase))
}

/// Leaf hashes with the contracts record: txs, the record (if any), the receipt (if any).
pub fn leaf_hashes_full(txs: &[Tx], contracts: Option<&ContractsRecord>, coinbase: Option<&Coinbase>) -> Vec<Hash> {
    let mut hs: Vec<Hash> = txs.iter().map(Tx::hash).collect();
    if let Some(c) = contracts {
        hs.push(c.hash());
    }
    if let Some(c) = coinbase {
        hs.push(c.hash());
    }
    hs
}

/// Leaf items with uncle records (docs/SPEED.md 3.3): txs, the contracts record (if any), the
/// uncle records in body order, the receipt (if any). Equal to [`leaf_hashes_full`] without uncles.
pub fn leaf_hashes_all(txs: &[Tx], contracts: Option<&ContractsRecord>, uncles: &[UncleRecord], coinbase: Option<&Coinbase>) -> Vec<Hash> {
    let mut hs: Vec<Hash> = txs.iter().map(Tx::hash).collect();
    if let Some(c) = contracts {
        hs.push(c.hash());
    }
    hs.extend(uncles.iter().map(UncleRecord::hash));
    if let Some(c) = coinbase {
        hs.push(c.hash());
    }
    hs
}

/// `tx_root` over [`leaf_hashes_all`].
pub fn tx_root_all(txs: &[Tx], contracts: Option<&ContractsRecord>, uncles: &[UncleRecord], coinbase: Option<&Coinbase>) -> Hash {
    merkle::root(&leaf_hashes_all(txs, contracts, uncles, coinbase))
}

/// A block's `tx_root` with its contracts record (equal to [`tx_root_with`] when none).
pub fn tx_root_full(txs: &[Tx], contracts: Option<&ContractsRecord>, coinbase: Option<&Coinbase>) -> Hash {
    merkle::root(&leaf_hashes_full(txs, contracts, coinbase))
}

/// [`leaf_path`] over [`leaf_hashes_full`].
pub fn leaf_path_full(txs: &[Tx], contracts: Option<&ContractsRecord>, coinbase: Option<&Coinbase>, index: usize) -> Vec<u8> {
    crate::contracts::path_bytes(&leaf_hashes_full(txs, contracts, coinbase), index)
}

/// tx_root path of leaf `index` (a tx index, or `txs.len()` for the receipt) as the
/// 33-byte entries the Solana verifier takes (`sibling [32] || sibling_is_left u8`).
pub fn leaf_path(txs: &[Tx], coinbase: Option<&Coinbase>, index: usize) -> Vec<u8> {
    let hs = leaf_hashes(txs, coinbase);
    let mut out = Vec::new();
    for (sib, left) in merkle::proof(&hs, index) {
        out.extend_from_slice(&sib);
        out.push(left as u8);
    }
    out
}
