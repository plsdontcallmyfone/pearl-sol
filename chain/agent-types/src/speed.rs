//! Chain speed upgrade (docs/SPEED.md section 3): activation height, uncle constants and
//! the uncle record.
//!
//! ```text
//! uncle record (137 bytes)
//! off len field
//!   0   1 version u8 = 0x03 (no tx 0x01, coinbase 0x00 or contracts record 0x02 starts so)
//!   1   8 height u64        (the including block)
//!   9  32 uncle_hash [32]
//!  41   8 uncle_height u64
//!  49  32 miner [32]        (the uncle header's payout key)
//!  81  32 job_id [32]       (the uncle header's job_id, never zero)
//! 113   8 reward u64        (credited to miner at mature_height)
//! 121   8 nephew u64        (credited to the including block's miner at mature_height)
//! 129   8 mature_height u64 (height + finality_k in force at the end of the block)
//! = 137 bytes
//! ```
//! `item hash = blake3(record)`; the record is a tx_root leaf after the txs and the contracts
//! record and before the coinbase receipt (block.rs).

use crate::codec::{R, W};
use crate::{blake3_hash, DecodeError, Hash, Header, Pubkey};

/// Cadre public devnet chain id (deploy/genesis.json).
pub const DEVNET_CHAIN_ID: u32 = 1_128_350_802;
/// Height of the block whose execution migrates the devnet to 30 s blocks (docs/SPEED.md 3.1).
pub const DEVNET_SPEED_HEIGHT: u64 = 6000;
/// Test-only chain ids: `TEST_SPEED_CHAIN_BASE + h` (h < 0x10000) migrate at height `h`, so tests
/// and soaks exercise the devnet migration at a low height. No real chain uses this range.
pub const TEST_SPEED_CHAIN_BASE: u32 = 0x7E57_0000;

pub const MAX_UNCLES: usize = 2;
pub const MAX_UNCLE_DEPTH: u64 = 2;
/// Nephew bonus: 1/32 of each uncle's reward.
pub const NEPHEW_DIVISOR: u64 = 32;

pub const UNCLE_VERSION: u8 = 0x03;
pub const UNCLE_RECORD_LEN: usize = 137;

/// First height at which the uncle rules apply on `chain_id`: the devnet's speed height (and
/// the test range's); every other chain has them from genesis.
pub fn speed_upgrade_height(chain_id: u32) -> u64 {
    speed_migration_height(chain_id).unwrap_or(0)
}

/// Height of the block that runs the one-time 60 s -> 30 s config migration, for the chains
/// that have one (the devnet and the test range). Other chains keep their genesis config.
pub fn speed_migration_height(chain_id: u32) -> Option<u64> {
    match chain_id {
        DEVNET_CHAIN_ID => Some(DEVNET_SPEED_HEIGHT),
        c if (TEST_SPEED_CHAIN_BASE..TEST_SPEED_CHAIN_BASE + 0x1_0000).contains(&c) => Some((c - TEST_SPEED_CHAIN_BASE) as u64),
        _ => None,
    }
}

/// Uncle share at depth `d` as (numerator, denominator): 1/2 at depth 1, 1/4 at depth 2.
pub fn uncle_share(depth: u64) -> Option<(u64, u64)> {
    match depth {
        1 => Some((1, 2)),
        2 => Some((1, 4)),
        _ => None,
    }
}

/// `(reward, nephew)` for an uncle at `depth` whose own subsidy would have been `base`.
pub fn uncle_reward(base: u64, depth: u64) -> Option<(u64, u64)> {
    let (n, d) = uncle_share(depth)?;
    let reward = ((base as u128 * n as u128) / d as u128) as u64;
    Some((reward, reward / NEPHEW_DIVISOR))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct UncleRecord {
    pub height: u64,
    pub uncle_hash: Hash,
    pub uncle_height: u64,
    pub miner: Pubkey,
    pub job_id: Hash,
    pub reward: u64,
    pub nephew: u64,
    pub mature_height: u64,
}

impl UncleRecord {
    pub fn encode(&self) -> [u8; UNCLE_RECORD_LEN] {
        let mut w = W::new();
        w.u8(UNCLE_VERSION)
            .u64(self.height)
            .bytes(&self.uncle_hash)
            .u64(self.uncle_height)
            .bytes(&self.miner)
            .bytes(&self.job_id)
            .u64(self.reward)
            .u64(self.nephew)
            .u64(self.mature_height);
        w.into_inner().try_into().expect("137 bytes")
    }

    pub fn decode(b: &[u8]) -> Result<Self, DecodeError> {
        let mut r = R::new(b);
        let v = r.u8()?;
        if v != UNCLE_VERSION {
            return Err(DecodeError::Version(v as u32));
        }
        let u = Self {
            height: r.u64()?,
            uncle_hash: r.arr()?,
            uncle_height: r.u64()?,
            miner: r.arr()?,
            job_id: r.arr()?,
            reward: r.u64()?,
            nephew: r.u64()?,
            mature_height: r.u64()?,
        };
        r.finish()?;
        Ok(u)
    }

    /// `blake3(record)`: this leaf's item hash in tx_root.
    pub fn hash(&self) -> Hash {
        blake3_hash(&[&self.encode()])
    }
}

/// An uncle carried in a block body: the stale header, its proof, and the record execution
/// produced for it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Uncle {
    pub header: Header,
    pub proof: Vec<u8>,
    pub record: UncleRecord,
}

impl Uncle {
    /// Context-free checks against the including block's height.
    pub fn check(&self, height: u64) -> Result<(), &'static str> {
        let r = &self.record;
        if r.height != height {
            return Err("uncle record height is not the block's");
        }
        if r.uncle_hash != self.header.hash() || r.uncle_height != self.header.height {
            return Err("uncle record does not match its header");
        }
        if r.miner != self.header.miner || r.job_id != self.header.job_id {
            return Err("uncle record miner or job does not match its header");
        }
        if self.header.job_id == [0; 32] {
            return Err("an uncle must be a job block");
        }
        if crate::block::proof_hash(&self.proof) != self.header.proof_hash {
            return Err("uncle proof_hash mismatch");
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn record_roundtrip_and_layout() {
        let r = UncleRecord {
            height: 0x0102,
            uncle_hash: [3; 32],
            uncle_height: 0x0101,
            miner: [4; 32],
            job_id: [5; 32],
            reward: 77,
            nephew: 2,
            mature_height: 0x0102 + 60,
        };
        let b = r.encode();
        assert_eq!(b.len(), UNCLE_RECORD_LEN);
        assert_eq!(b[0], 0x03);
        assert_eq!(&b[1..9], &0x0102u64.to_le_bytes());
        assert_eq!(&b[113..121], &77u64.to_le_bytes());
        assert_eq!(UncleRecord::decode(&b).unwrap(), r);
        let mut bad = b;
        bad[0] = 0;
        assert!(UncleRecord::decode(&bad).is_err());
        assert!(UncleRecord::decode(&b[..136]).is_err());
        assert_eq!(r.hash(), blake3_hash(&[&b]));
    }

    #[test]
    fn heights_and_shares() {
        assert_eq!(DEVNET_SPEED_HEIGHT, 6000);
        assert_eq!(speed_upgrade_height(DEVNET_CHAIN_ID), DEVNET_SPEED_HEIGHT);
        assert_eq!(speed_migration_height(DEVNET_CHAIN_ID), Some(DEVNET_SPEED_HEIGHT));
        assert_eq!(speed_upgrade_height(1), 0);
        assert_eq!(speed_migration_height(1), None);
        assert_eq!(speed_migration_height(TEST_SPEED_CHAIN_BASE + 12), Some(12));
        assert_eq!(uncle_reward(1000, 1), Some((500, 15)));
        assert_eq!(uncle_reward(1000, 2), Some((250, 7)));
        assert_eq!(uncle_reward(1000, 3), None);
        assert_eq!(uncle_reward(1000, 0), None);
    }
}
