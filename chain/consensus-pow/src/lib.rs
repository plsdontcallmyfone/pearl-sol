//! Useful-PoW consensus for the agent chain (SPEC 3): difficulty, proof verification
//! seam, and the chain manager (block tree, fork choice, finality, reorgs).

pub mod chain;
pub mod difficulty;
pub mod verifier;

pub use chain::{tx_accounts, Accept, BODY_ASK_TIMEOUT_MS, BODY_RETRY_SHARE, BODY_WINDOW, Chain, ChainError, ChainEvent, Genesis, HeaderAccept, HistoryEntry, JobChoice, LiveNode, Rewards, RouteTxEntry, Template, MAX_ORPHAN_BYTES, TIME_TOO_NEW, is_time_too_new};
pub use chain::repair::Repair;
pub use chain::headers::{check_header_links, decode_header_batch, encode_header_batch, is_header_batch, HeaderBatchAccept, HEADER_BATCH_MAGIC, MAX_HEADER_BATCH, MAX_HEADER_BATCH_BYTES};
pub use verifier::{MockVerifier, PearlVerifier, PowVerifier};
pub use chain::weak::{is_weak_msg, WeakBlock, WeakReject, MAX_WEAK_MSG, WEAK_PARENT_DEPTH};
