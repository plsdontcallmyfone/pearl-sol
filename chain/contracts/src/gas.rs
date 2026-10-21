//! Gas schedule (consensus rule; docs/CONTRACTS.md "Gas").
//!
//! One gas = one unit of wasmi fuel. Wasm instructions are charged by wasmi's own fuel
//! costs (pinned with the wasmi version); host calls are charged here.
//!
//! Measured on the test box (8 vCPU x86_64, release build, 2026-10-21, docs/CONTRACTS.md
//! "Measured"): wasm runs at 0.77 ns/gas (an arithmetic loop) to 7.36 ns/gas (the worst case
//! found: a `call_indirect` loop, because wasmi 1.1's fuel table charges every op 1); a
//! tight `br` loop 2.25, `i64.div_u` 3.14, `memory.copy` 4.88 ns/gas. Compiling a module costs
//! 26.6-29.8 ns/byte, instantiating a cached one 4.9 ns/byte; merkleizing a 64-byte state
//! write about 3.0 us. Host costs are set at or above those times at 2.25 ns/gas, and the
//! block gas limit is sized for the 7.36 ns/gas worst case.
//!
//! Changing any value is a consensus change (it needs an activation height).

/// Loading a contract (per byte of its code): parse + translate + instantiate. Charged on
/// every call frame, whether the node had the module cached or not.
pub const LOAD_PER_CODE_BYTE: u64 = 20;
/// Validating code at DeployCode (per byte): one full parse and translation.
pub const DEPLOY_PER_CODE_BYTE: u64 = 40;
/// Any host call.
pub const HOST_BASE: u64 = 200;
/// Copying bytes between the host and wasm memory (input, scratch, return, args).
pub const COPY_PER_BYTE: u64 = 1;
/// A storage read (state lookup), plus per byte read.
pub const STORAGE_READ: u64 = 10_000;
pub const STORAGE_READ_PER_BYTE: u64 = 2;
/// A storage write or remove (state write, merkleized at block end), plus per byte written.
pub const STORAGE_WRITE: u64 = 20_000;
pub const STORAGE_WRITE_PER_BYTE: u64 = 20;
/// An event, plus per byte of topics and data (events live in state and in the block's events_root).
pub const EMIT: u64 = 5_000;
pub const EMIT_PER_BYTE: u64 = 20;
/// Native balance read and transfer.
pub const BALANCE: u64 = 20_000;
pub const TRANSFER: u64 = 60_000;
/// A cross-contract call frame (on top of the callee's load cost).
pub const CALL: u64 = 50_000;
/// Native module read (account, job, agent, profile, follow) and native call (a tx body
/// executed as the contract), plus per byte of the body.
pub const NATIVE_READ: u64 = 30_000;
pub const NATIVE_CALL: u64 = 200_000;
pub const NATIVE_CALL_PER_BYTE: u64 = 50;
/// Hashing and signature checks.
pub const HASH: u64 = 1_000;
pub const HASH_PER_BYTE: u64 = 4;
pub const ED25519_VERIFY: u64 = 150_000;
/// Debug log line (kept only by simulations; no state effect).
pub const LOG: u64 = 1_000;
