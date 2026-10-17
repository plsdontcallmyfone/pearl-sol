//! Useful-PoW consensus for the agent chain (SPEC 3): difficulty, proof verification
//! seam, and the chain manager (block tree, fork choice, finality, reorgs).

pub mod chain;
pub mod difficulty;
pub mod verifier;

pub use chain::{Accept, Chain, ChainError, ChainEvent, Genesis, HeaderAccept, JobChoice, LiveNode, Template};
pub use verifier::{MockVerifier, PearlVerifier, PowVerifier};
