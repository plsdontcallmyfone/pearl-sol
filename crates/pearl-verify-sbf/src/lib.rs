//! Pearl block verification for the Solana light client (SPEC v2 5.3).
//!
//! `no_std` (+ `alloc`), no external hash crates: a small BLAKE3 core with resumable
//! tree state, the PSJ1 plain-proof checks split into steps, fallback-matrix
//! commitments rebuilt in resumable ranges, Pearl's WTEMA retarget, Commonware MMR
//! inclusion proofs (SHA-256 supplied by the caller: the Solana syscall on chain) and
//! the chain's `tx_root` paths. The native reference is crates/pearl-job and
//! chain/consensus-pow; tests/ cross-check against both.

#![no_std]

pub mod blake3;
pub mod chaintx;
pub mod config;
pub mod difficulty;
pub mod fallback;
pub mod header;
pub mod mmr;
pub mod proof;
pub mod verify;

pub use primitive_types::U256;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Error {
    Config,
    Shape,
    ProofFormat,
    Nonce,
    Dims,
    Tile,
    Opening,
    Commitment,
    Int7,
    Target,
    MmrFormat,
    MmrProof,
    MmrRoot,
    TxPath,
    Tx,
}

impl Error {
    pub fn code(self) -> u32 {
        self as u32
    }
}
