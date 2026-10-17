//! 256-byte block header (SPEC 3.1, PROOF-FORMAT.md 7.1).

use crate::blake3;

pub const LEN: usize = 256;
pub const VERSION: u32 = 1;

pub mod off {
    pub const VERSION: usize = 0;
    pub const HEIGHT: usize = 4;
    pub const PREV_HASH: usize = 12;
    pub const TIMESTAMP_MS: usize = 44;
    pub const TARGET_BITS: usize = 52;
    pub const JOB_ID: usize = 56;
    pub const MINER: usize = 88;
    pub const TX_ROOT: usize = 120;
    pub const STATE_ROOT: usize = 152;
    pub const WITHDRAW_ROOT: usize = 184;
    pub const PROOF_HASH: usize = 216;
    pub const NONCE: usize = 248;
}

#[derive(Clone, Copy)]
pub struct Header<'a>(pub &'a [u8; LEN]);

impl<'a> Header<'a> {
    fn u32_at(&self, o: usize) -> u32 {
        u32::from_le_bytes(self.0[o..o + 4].try_into().unwrap())
    }
    fn u64_at(&self, o: usize) -> u64 {
        u64::from_le_bytes(self.0[o..o + 8].try_into().unwrap())
    }
    fn h_at(&self, o: usize) -> [u8; 32] {
        self.0[o..o + 32].try_into().unwrap()
    }
    pub fn version(&self) -> u32 {
        self.u32_at(off::VERSION)
    }
    pub fn height(&self) -> u64 {
        self.u64_at(off::HEIGHT)
    }
    pub fn prev_hash(&self) -> [u8; 32] {
        self.h_at(off::PREV_HASH)
    }
    pub fn timestamp_ms(&self) -> u64 {
        self.u64_at(off::TIMESTAMP_MS)
    }
    pub fn target_bits(&self) -> u32 {
        self.u32_at(off::TARGET_BITS)
    }
    pub fn job_id(&self) -> [u8; 32] {
        self.h_at(off::JOB_ID)
    }
    pub fn is_fallback(&self) -> bool {
        self.0[off::JOB_ID..off::JOB_ID + 32].iter().all(|&b| b == 0)
    }
    pub fn miner(&self) -> [u8; 32] {
        self.h_at(off::MINER)
    }
    pub fn tx_root(&self) -> [u8; 32] {
        self.h_at(off::TX_ROOT)
    }
    pub fn state_root(&self) -> [u8; 32] {
        self.h_at(off::STATE_ROOT)
    }
    pub fn withdraw_root(&self) -> [u8; 32] {
        self.h_at(off::WITHDRAW_ROOT)
    }
    pub fn proof_hash(&self) -> [u8; 32] {
        self.h_at(off::PROOF_HASH)
    }
    pub fn nonce(&self) -> u64 {
        self.u64_at(off::NONCE)
    }
    /// `blake3(header)`.
    pub fn block_hash(&self) -> [u8; 32] {
        blake3::hash(self.0)
    }
    /// `blake3(header with proof_hash zeroed)`.
    pub fn pow_digest(&self) -> [u8; 32] {
        let mut h = *self.0;
        h[off::PROOF_HASH..off::PROOF_HASH + 32].fill(0);
        blake3::hash(&h)
    }
}
