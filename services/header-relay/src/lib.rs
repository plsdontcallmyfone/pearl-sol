//! header-relay as a library (the binary is src/main.rs): node and Solana access, the
//! relay pipeline, proof-mode settlement.

pub mod chain;
pub mod ledger;
pub mod relay;
pub mod settle;
pub mod sol;

/// eprintln! with the Solana RPC URL (which may carry a provider key) cut to scheme://host.
#[macro_export]
macro_rules! rlog {
    ($($t:tt)*) => { eprintln!("{}", $crate::sol::redact(&format!($($t)*))) };
}
