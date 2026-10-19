//! RejectResult records (SPEC 4.1, owner decision 2026-10-18).
//!
//! A rejected delivery is kept forever, keyed by job and index, plus a per-job count and
//! a per-deliverer tally. These are the only reputation facts the chain stores; every
//! other profile figure is derived by an indexer from jobs, txs and coinbase receipts.
//!
//! ```text
//! Reject       ["reject", job_id || index u32 LE]  tag 14 | deliverer [32] | result_hash [32]
//!              | height u64 | protocol_fee u64 | count u8 | signer [32]*
//! RejectCount  ["reject-count", job_id]            tag 15 | count u32
//! RejectStats  ["reject-stats", deliverer]         tag 16 | rejected u64 | rejected_fee u64
//!              | first_height u64 | last_height u64
//! ```

use agent_types::codec::{R, W};
use agent_types::{DecodeError, Hash, Pubkey};

use crate::schema::{key, tag, Key};

pub fn reject_key(job_id: &Hash, index: u32) -> Key {
    let mut id = job_id.to_vec();
    id.extend_from_slice(&index.to_le_bytes());
    key(tag::REJECT, &id)
}

pub fn reject_count_key(job_id: &Hash) -> Key {
    key(tag::REJECT_COUNT, job_id)
}

pub fn reject_stats_key(deliverer: &Pubkey) -> Key {
    key(tag::REJECT_STATS, deliverer)
}

fn expect(r: &mut R<'_>, t: u8) -> Result<(), DecodeError> {
    if r.u8()? != t {
        return Err(DecodeError::Invalid("record tag"));
    }
    Ok(())
}

/// One rejected delivery.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Reject {
    pub deliverer: Pubkey,
    pub result_hash: Hash,
    /// Height of the block holding the RejectResult.
    pub height: u64,
    /// The job's protocol fee P (the weight profiles use).
    pub protocol_fee: u64,
    /// Evaluator keys whose signatures the RejectResult carried.
    pub signers: Vec<Pubkey>,
}

impl Reject {
    pub fn encode(&self) -> Vec<u8> {
        let mut w = W::new();
        w.u8(tag::REJECT).bytes(&self.deliverer).bytes(&self.result_hash).u64(self.height).u64(self.protocol_fee);
        w.u8(self.signers.len() as u8);
        for s in &self.signers {
            w.bytes(s);
        }
        w.into_inner()
    }
    pub fn decode(b: &[u8]) -> Result<Self, DecodeError> {
        let mut r = R::new(b);
        expect(&mut r, tag::REJECT)?;
        let deliverer = r.arr()?;
        let result_hash = r.arr()?;
        let height = r.u64()?;
        let protocol_fee = r.u64()?;
        let n = r.u8()? as usize;
        let mut signers = Vec::with_capacity(n);
        for _ in 0..n {
            signers.push(r.arr()?);
        }
        r.finish()?;
        Ok(Self { deliverer, result_hash, height, protocol_fee, signers })
    }
}

pub fn encode_count(n: u32) -> Vec<u8> {
    let mut w = W::new();
    w.u8(tag::REJECT_COUNT).u32(n);
    w.into_inner()
}

pub fn decode_count(b: &[u8]) -> Result<u32, DecodeError> {
    let mut r = R::new(b);
    expect(&mut r, tag::REJECT_COUNT)?;
    let n = r.u32()?;
    r.finish()?;
    Ok(n)
}

/// Rejections recorded against one deliverer, over all jobs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct RejectStats {
    pub rejected: u64,
    /// Sum of P over the rejected deliveries.
    pub rejected_fee: u64,
    pub first_height: u64,
    pub last_height: u64,
}

impl RejectStats {
    pub fn encode(&self) -> Vec<u8> {
        let mut w = W::new();
        w.u8(tag::REJECT_STATS).u64(self.rejected).u64(self.rejected_fee).u64(self.first_height).u64(self.last_height);
        w.into_inner()
    }
    pub fn decode(b: &[u8]) -> Result<Self, DecodeError> {
        let mut r = R::new(b);
        expect(&mut r, tag::REJECT_STATS)?;
        let s = Self { rejected: r.u64()?, rejected_fee: r.u64()?, first_height: r.u64()?, last_height: r.u64()? };
        r.finish()?;
        Ok(s)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip() {
        let r = Reject { deliverer: [1; 32], result_hash: [2; 32], height: 9, protocol_fee: 77, signers: vec![[3; 32], [4; 32]] };
        assert_eq!(Reject::decode(&r.encode()).unwrap(), r);
        assert_eq!(decode_count(&encode_count(5)).unwrap(), 5);
        let s = RejectStats { rejected: 2, rejected_fee: 10, first_height: 3, last_height: 8 };
        assert_eq!(RejectStats::decode(&s.encode()).unwrap(), s);
        assert_ne!(reject_key(&[0; 32], 0), reject_key(&[0; 32], 1));
    }
}
