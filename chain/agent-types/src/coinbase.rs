//! Coinbase receipt (Agent Credit request R1): the LAST leaf of every non-genesis
//! block's `tx_root`, so a block's reward is provable by tx_root inclusion.
//!
//! ```text
//! off len field
//!   0   1 version u8 = 0x00 (a tx starts with TX_VERSION = 0x01, so a receipt never decodes as a tx)
//!   1   8 height u64
//!   9  32 miner [32] (the header's payout key)
//!  41   8 fees u64       (sum of the block's tx fees, chain base units)
//!  49   8 subsidy u64    (reserve subsidy of a job block; 0 for a fallback block)
//!  57  32 job_id [32]    (the header's job_id; zero for a fallback block)
//!  89   8 mature_height u64 (height + finality_k in force at the end of the block:
//!                         fees + subsidy are credited to `miner` when that block executes)
//! = 97 bytes
//! ```
//! `receipt_hash = blake3(record)`; its tx_root leaf is `blake3(0x00 || receipt_hash)`,
//! the same leaf rule as a tx hash (merkle.rs). height, miner and job_id repeat header
//! fields so a verifier checks them against the header it holds.

use crate::codec::{R, W};
use crate::{blake3_hash, DecodeError, Hash, Pubkey};

pub const COINBASE_VERSION: u8 = 0;
pub const COINBASE_LEN: usize = 97;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Coinbase {
    pub height: u64,
    pub miner: Pubkey,
    pub fees: u64,
    pub subsidy: u64,
    pub job_id: Hash,
    pub mature_height: u64,
}

impl Coinbase {
    pub fn encode(&self) -> [u8; COINBASE_LEN] {
        let mut w = W::new();
        w.u8(COINBASE_VERSION)
            .u64(self.height)
            .bytes(&self.miner)
            .u64(self.fees)
            .u64(self.subsidy)
            .bytes(&self.job_id)
            .u64(self.mature_height);
        w.into_inner().try_into().expect("97 bytes")
    }

    pub fn decode(b: &[u8]) -> Result<Self, DecodeError> {
        let mut r = R::new(b);
        let v = r.u8()?;
        if v != COINBASE_VERSION {
            return Err(DecodeError::Version(v as u32));
        }
        let c = Self {
            height: r.u64()?,
            miner: r.arr()?,
            fees: r.u64()?,
            subsidy: r.u64()?,
            job_id: r.arr()?,
            mature_height: r.u64()?,
        };
        r.finish()?;
        Ok(c)
    }

    /// `blake3(record)`: the value the tx_root tree takes as this leaf's "tx hash".
    pub fn hash(&self) -> Hash {
        blake3_hash(&[&self.encode()])
    }

    /// Total reward credited at `mature_height`.
    pub fn reward(&self) -> u64 {
        self.fees.saturating_add(self.subsidy)
    }
}
