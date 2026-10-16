//! Thin agent-chain access for the pearl-sol services (miner, attester).
//!
//! Deliberately avoids the solana-sdk / solana-client crates (build time): raw
//! JSON-RPC over HTTP, a legacy-message transaction builder, PDA derivation, and
//! Borsh decoders for the `jobs` and `mining` Anchor accounts in agent/programs.
//! If those programs change their account layouts, `accounts.rs` must follow.

pub mod accounts;
pub mod ix;
pub mod key;
pub mod rpc;
pub mod tx;

pub use key::{find_program_address, read_keypair, Pubkey};
pub use rpc::Rpc;

/// Program IDs from agent/Anchor.toml (localnet keys in keys/localnet/).
pub const JOBS_PROGRAM: &str = "7e9pEhjwEqKemjC6zUugp4isi2zQMvaZPhn9kdnDuzqL";
pub const MINING_PROGRAM: &str = "3GrrgQMSHC3PjkW6UmvQJPGK1WBvhxiz3tkigafj3GhP";
pub const DEFAULT_RPC: &str = "http://127.0.0.1:10630";
