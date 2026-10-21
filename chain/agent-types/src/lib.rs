//! Agent-chain wire types (SPEC v2 sections 2 and 3.1).
//!
//! Everything here uses the SPEC's fixed little-endian binary layout, not a varint
//! codec, so the wallet extension and the Solana light client can build and parse
//! headers and transactions with plain byte slicing. `commonware_codec` impls wrap
//! the same bytes so the types travel over Commonware p2p and resolver channels.

pub mod block;
pub mod coinbase;
pub mod codec;
pub mod contracts;
pub mod header;
pub mod merkle;
pub mod social;
pub mod tx;

pub use block::Block;
pub use coinbase::Coinbase;
pub use header::Header;
pub use social::{FollowEdge, Post, Reaction, SetProfile};
pub use tx::{AttesterSig, MatrixSpec, PostJob, RegisterModel, Segment, Tx, TxBody, TxKind};

use thiserror::Error;

pub type Pubkey = [u8; 32];
pub type Hash = [u8; 32];
pub type Sig = [u8; 64];

/// Transaction signature namespace (Commonware `sign(ns, msg)` framing).
pub const NS_TX: &[u8] = b"pearl-sol:tx:v1";
// Attester and evaluator messages are signed as RAW ed25519 over `domain || fields`
// (no Commonware framing), the same bytes the Solana programs check with the ed25519
// precompile and the relayer already signs.

/// Evaluator signatures for CompleteJob: `domain || job_id || deliverer || result_hash`.
pub const DOMAIN_RESULT: &[u8] = b"pearl-sol:result:v1";
/// Evaluator signatures for RejectResult (SPEC 4.1): `domain || job_id || deliverer || result_hash`.
pub const DOMAIN_REJECT: &[u8] = b"pearl-sol:reject:v1";
/// Plain BridgeUnlock: `domain || solana_nonce u64 LE || recipient || amount_spl u64 LE`.
pub const DOMAIN_DEPOSIT: &[u8] = b"pearl-sol:deposit:v1";
/// Storefront order BridgeUnlock (job_order = 1):
/// `domain || solana_nonce u64 LE || recipient || amount_spl u64 LE || sha256(PostJob body)`.
pub const DOMAIN_DEPOSIT_ORDER: &[u8] = b"pearl-sol:deposit-order:v1";
/// Order job id: `sha256(domain || solana_nonce u64 LE)`.
pub const DOMAIN_ORDER_JOB: &[u8] = b"pearl-sol:order-job:v1";

/// ClearRoute release signature: `domain || chain_id u32 LE || account || route_nonce u64 LE`.
pub const DOMAIN_ROUTE_CLEAR: &[u8] = b"pearl-sol:route-clear:v1";

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

pub fn sha256(parts: &[&[u8]]) -> Hash {
    use sha2::Digest;
    let mut h = sha2::Sha256::new();
    for p in parts {
        h.update(p);
    }
    h.finalize().into()
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
