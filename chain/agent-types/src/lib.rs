//! Agent-chain wire types (SPEC v2 sections 2 and 3.1).
//!
//! Everything here uses the SPEC's fixed little-endian binary layout, not a varint
//! codec, so the wallet extension and the Solana light client can build and parse
//! headers and transactions with plain byte slicing. `commonware_codec` impls wrap
//! the same bytes so the types travel over Commonware p2p and resolver channels.

pub mod block;
pub mod codec;
pub mod header;
pub mod merkle;
pub mod tx;

pub use block::Block;
pub use header::Header;
pub use tx::{AttesterSig, PostJob, Tx, TxBody, TxKind};

use thiserror::Error;

pub type Pubkey = [u8; 32];
pub type Hash = [u8; 32];
pub type Sig = [u8; 64];

/// Transaction signature namespace (Commonware `sign(ns, msg)` framing).
pub const NS_TX: &[u8] = b"pearl-sol:tx:v1";
/// Evaluator signatures for CompleteJob: msg = job_id || deliverer || result_hash.
pub const NS_RESULT: &[u8] = b"pearl-sol:result:v1";
/// Attester signatures for a plain BridgeUnlock: msg = solana_nonce u64 LE || recipient || amount_spl u64 LE.
pub const NS_DEPOSIT: &[u8] = b"pearl-sol:deposit:v1";
/// Attester signatures for a job-order BridgeUnlock:
/// msg = solana_nonce u64 LE || recipient || amount_spl u64 LE || blake3(PostJob body bytes).
pub const NS_DEPOSIT_JOB: &[u8] = b"pearl-sol:deposit-job:v1";

/// Native base units per token (9 decimals).
pub const UNITS_PER_TOKEN: u64 = 1_000_000_000;
/// Whole supply on the chain: 1B tokens x 1e9 base units, all in the escrow at genesis.
pub const TOTAL_SUPPLY: u64 = 1_000_000_000 * UNITS_PER_TOKEN;
/// 1 Solana SPL base unit (6 dp) = 1000 chain base units (9 dp).
pub const SPL_SCALE: u64 = 1000;

pub const TX_VERSION: u8 = 1;
pub const HEADER_VERSION: u32 = 1;

/// Reserved account holding the bridge escrow. No key controls it.
pub fn escrow_address() -> Pubkey {
    *blake3::hash(b"pearl-sol:escrow:v1").as_bytes()
}

/// Reserved account holding the mining reserve pool. No key controls it.
pub fn pool_address() -> Pubkey {
    *blake3::hash(b"pearl-sol:mining-pool:v1").as_bytes()
}

pub fn is_reserved(addr: &Pubkey) -> bool {
    *addr == escrow_address() || *addr == pool_address()
}

pub fn blake3_hash(parts: &[&[u8]]) -> Hash {
    let mut h = blake3::Hasher::new();
    for p in parts {
        h.update(p);
    }
    *h.finalize().as_bytes()
}

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum DecodeError {
    #[error("truncated at byte {0}")]
    Truncated(usize),
    #[error("trailing bytes: {0}")]
    Trailing(usize),
    #[error("unsupported version {0}")]
    Version(u32),
    #[error("unknown tx kind {0}")]
    Kind(u8),
    #[error("invalid field: {0}")]
    Invalid(&'static str),
    #[error("too large: {0}")]
    TooLarge(&'static str),
}
