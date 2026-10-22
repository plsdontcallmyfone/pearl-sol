//! Blob data-lifecycle parameters (SPEC 4.5 "Segment data retention", 2026-10-22).
//!
//! Like the DM parameters these live in their OWN record, not in the config record, so the
//! config record's encoding never changes because of them. A missing record means every
//! parameter holds its default, so a chain where no admin set one has the same state root as
//! before this record existed.
//!
//! ```text
//! BlobParams ["blob-params"]   tag 52 | version u8 = 1 | segment_replay_blocks u32
//! ```
//!
//! The parameters are consensus state (every node reads the same value) but only node policy
//! reads them: which blobs a node keeps. Consensus never reads a blob.

use agent_types::codec::{R, W};
use agent_types::DecodeError;

use crate::schema::{key, Key};

/// Schema tag of the record (36-39 and 49-51 are left free for rows working in parallel).
pub const TAG: u8 = 52;

/// AdminConfig keys of the blob parameters (valid from [`blob_params_upgrade_height`]).
pub mod keys {
    /// u32 blocks: how long a node keeps a segment's A blob after its ticket closed (and how
    /// long past the deadline of a ticket that never closed), for evaluator replay and fraud
    /// proofs.
    pub const SEGMENT_REPLAY_BLOCKS: u8 = 48;
    pub const FIRST: u8 = 48;
    pub const LAST: u8 = 48;
    /// Key 49 is reserved for a later blob parameter.
    pub const RESERVED_LAST: u8 = 49;

    pub fn name(k: u8) -> &'static str {
        match k {
            SEGMENT_REPLAY_BLOCKS => "segment_replay_blocks",
            _ => "unknown",
        }
    }
}

/// Cadre public devnet chain id (deploy/genesis.json).
pub const DEVNET_CHAIN_ID: u32 = 1_128_350_802;
/// Coordinated activation height of AdminConfig keys 48-49 on the public devnet (tip about
/// 5011 at 2026-10-22 02:15 UTC, about one block a minute). Below it a node refuses those keys
/// exactly as an older node does ("unknown config key"); every other chain has them from
/// genesis. Only setting a key needs the upgrade: the default applies everywhere before.
pub const DEVNET_BLOB_PARAMS_HEIGHT: u64 = 5800;

/// First height at which keys 48-49 are valid on `chain_id`.
pub fn blob_params_upgrade_height(chain_id: u32) -> u64 {
    match chain_id {
        DEVNET_CHAIN_ID => DEVNET_BLOB_PARAMS_HEIGHT,
        _ => 0,
    }
}

/// Is `k` an AdminConfig key handled by the blob parameters (48..=49)?
pub fn is_blob_key(k: u8) -> bool {
    (keys::FIRST..=keys::RESERVED_LAST).contains(&k)
}

pub fn blob_params_key() -> Key {
    key(TAG, &[])
}

/// Default `segment_replay_blocks`: 720 blocks, half a day of 60 s blocks.
pub const DEFAULT_SEGMENT_REPLAY_BLOCKS: u32 = 720;
/// Upper bound of `segment_replay_blocks` (about two years of 60 s blocks).
pub const MAX_SEGMENT_REPLAY_BLOCKS: u32 = 1_000_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BlobParams {
    pub segment_replay_blocks: u32,
}

impl Default for BlobParams {
    fn default() -> Self {
        Self { segment_replay_blocks: DEFAULT_SEGMENT_REPLAY_BLOCKS }
    }
}

impl BlobParams {
    pub fn encode(&self) -> Vec<u8> {
        let mut w = W::new();
        w.u8(TAG).u8(1).u32(self.segment_replay_blocks);
        w.into_inner()
    }

    pub fn decode(b: &[u8]) -> Result<Self, DecodeError> {
        let mut r = R::new(b);
        if r.u8()? != TAG || r.u8()? != 1 {
            return Err(DecodeError::Invalid("blob params tag"));
        }
        let p = Self { segment_replay_blocks: r.u32()? };
        r.finish()?;
        Ok(p)
    }

    /// Encoded value of key `k` (empty for an unknown key).
    pub fn value(&self, k: u8) -> Vec<u8> {
        match k {
            keys::SEGMENT_REPLAY_BLOCKS => self.segment_replay_blocks.to_le_bytes().to_vec(),
            _ => Vec::new(),
        }
    }

    /// Set key `k` from its encoded value, checking its range.
    pub fn set(&mut self, k: u8, v: &[u8]) -> Result<(), String> {
        match k {
            keys::SEGMENT_REPLAY_BLOCKS => {
                let x = u32::from_le_bytes(v.try_into().map_err(|_| "expected u32".to_string())?);
                if x > MAX_SEGMENT_REPLAY_BLOCKS {
                    return Err(format!("segment_replay_blocks must be 0..={MAX_SEGMENT_REPLAY_BLOCKS}"));
                }
                self.segment_replay_blocks = x;
            }
            _ => return Err(format!("config key {k} is reserved")),
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_defaults_and_bounds() {
        let d = BlobParams::default();
        assert_eq!(d.segment_replay_blocks, 720);
        assert_eq!(BlobParams::decode(&d.encode()).unwrap(), d);
        let mut p = d;
        p.set(keys::SEGMENT_REPLAY_BLOCKS, &1440u32.to_le_bytes()).unwrap();
        assert_eq!(BlobParams::decode(&p.encode()).unwrap().segment_replay_blocks, 1440);
        assert_eq!(p.value(keys::SEGMENT_REPLAY_BLOCKS), 1440u32.to_le_bytes().to_vec());
        assert!(p.set(keys::SEGMENT_REPLAY_BLOCKS, &(MAX_SEGMENT_REPLAY_BLOCKS + 1).to_le_bytes()).is_err());
        assert!(p.set(keys::SEGMENT_REPLAY_BLOCKS, &[1, 2]).is_err());
        assert!(p.set(49, &[0; 4]).unwrap_err().contains("reserved"));
        assert!(is_blob_key(48) && is_blob_key(49) && !is_blob_key(47) && !is_blob_key(50));
        assert!(!crate::dm::is_dm_key(48) && !crate::dm::is_dm_key(49));
        assert!(48 > crate::config::keys::LAST);
        assert_eq!(blob_params_upgrade_height(DEVNET_CHAIN_ID), DEVNET_BLOB_PARAMS_HEIGHT);
        assert_eq!(blob_params_upgrade_height(11), 0);
        // the tag is not used by the main schema
        assert!(TAG > crate::schema::tag::CHANNEL_COUNT);
    }
}
