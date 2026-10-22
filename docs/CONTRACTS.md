# Cadre smart contracts

Status: built 2026-10-21 (branch `contracts`, merged to main with this doc). Owner request
2026-10-21: "take control and allow for smart contract builds, we'll also increase speed at
some point as well." Anyone can deploy their own program on Cadre; until now the chain had
only its built-in tx kinds (SPEC 2).

Contents: 1 decision, 2 tx kinds, 3 contract model, 4 ABI and host functions, 5 gas and fees,
6 native access, 7 state commitment and proofs, 8 activation and rollout, 9 security, 10
speed, 11 tests and measurements, 12 open items. Sources for every outside figure are in
research/CONTRACTS-VM.md (cited below by section, e.g. [R1]).

## 1. Decision: WebAssembly on the wasmi interpreter, Rust SDK

| | WASM (wasmi interpreter) | WASM (wasmtime / wasmer JIT) | EVM (revm) | SVM / sBPF (solana-sbpf) |
|---|---|---|---|---|
| Determinism | Interpreter, no native code generation. Floats rejected at deploy, SIMD not compiled in, fixed stack limits, fuel is exact per op | Needs NaN canonicalization and careful config; JIT output can differ by CPU feature set [R2] | Deterministic by design | Deterministic interpreter; JIT is the fast path on Solana |
| Metering | Built-in fuel (`consume_fuel`), resumable on host calls and out of fuel [R1] | Fuel or epoch interruption; a 2026 advisory was a fuel-accounting amplification bug [R2] | Gas per opcode, mature | Compute units, mature |
| Fit with our state | Host calls suspend wasm (`ResumableCallHostTrap`), so contract storage reads go straight to our async QMDB overlay | Same ABI possible, but needs sync state access or a thread bridge | Needs an EVM state trie view, 20-byte addresses, secp256k1 `ecrecover` | Requires every account a call touches declared up front: our contracts read storage, jobs and profiles dynamically |
| Accounts | Our ed25519 keys and 32-byte addresses as they are | Same | 20-byte H160 addresses and secp256k1 signatures: a second account type plus a mapping (Sei, Neon, pallet-revive all do this) [R7] | ed25519, 32-byte keys (matches) |
| Tooling | Rust + `cargo build --target wasm32-unknown-unknown`, our SDK; any language that emits wasm32 without floats | Same | Solidity, Foundry, MetaMask, ethers: the largest ecosystem | Anchor, Solana tooling; programs expect the Solana runtime and sysvars |
| AI agents writing contracts | Rust: the compiler and the type system catch whole bug classes before deploy; the SDK is small enough to read in one sitting. No study compares LLM bug rates for Rust and Solidity contracts (searched, not found [R11]); LLM-written Solidity "frequently exhibit[s] severe security flaws" [R11] | Same | Most LLM training data, and most of the known LLM contract vulnerabilities | Anchor is familiar to models, but the account model is easy to get wrong |
| Audit surface | wasmi (pure Rust, two public audits: SRLabs, Runtime Verification [R1]) + our host (one file of host functions, one driver) | A JIT compiler in consensus: Cranelift sandbox escapes in 2023 and 2026 [R2]; NEAR measured 7.6 s compiles of crafted contracts [R2] | revm plus a translation layer for accounts, gas and native modules | rbpf plus its JIT, and the Solana runtime model; two network-halting Solana loader bugs in 2024 [R8] |
| Who runs it on chain | Soroban (Stellar) runs wasmi and rejects a JIT for the reasons above [R1]; pallet-contracts ran wasmi before moving to PolkaVM [R5] | CosmWasm (wasmer), NEAR (wasmtime since nearcore 2.12), Stylus (wasmer) [R2, R3, R6] | Most EVM L1s and L2s | Solana, Eclipse, Sonic |

**Choice: WASM on wasmi 1.1.0, pinned exactly** (`chain/contracts/Cargo.toml`). It is the
smallest audit surface that still gives a real general-purpose VM, it fits our ed25519
accounts and async state without translation layers, and Rust contracts are what our own
code, our agents and our SDK already speak. wasmi 1.1.0 (2026-06-12) is past both published
wasmi advisories (CVE-2025-66627 fixed in 1.0.1; CVE-2024-28123 fixed in 0.31.1 [R1]); 2.0.0
(2026-09-01) adds a per-operator fuel table (`OperatorCost`) and is the planned upgrade (12).

EVM is the strongest alternative for developer reach (Solidity, MetaMask). It is not ruled
out: it can be added later as a second VM next to WASM, with its own address mapping, if the
owner wants Ethereum tooling. SVM was weighed as a contract VM (not a base chain): the
ed25519 match is real, but its declared-accounts model does not fit contracts that read our
native records on the fly, and its fast path is a JIT. Neither is built.

## 2. Tx kinds (SPEC 2 table, kinds 30-34)

All little-endian, the usual tx envelope (SPEC 2). Layouts in `chain/agent-types/src/contracts.rs`,
byte vectors in `chain/agent-types/vectors/contracts.json`.

| kind | Name | Body |
|---|---|---|
| 30 | DeployCode | `gas_limit u64 \| code (u32 len, <= 256 KiB codec cap, config max_code_size)` |
| 31 | Instantiate | `code_hash [32] \| admin [32] \| value u64 \| gas_limit u64 \| label (u8 len, <= 64) \| args (u16 len, <= 8192)` |
| 32 | Call | `contract [32] \| value u64 \| gas_limit u64 \| args (u16 len, <= 8192)` |
| 33 | Migrate | `contract [32] \| code_hash [32] \| gas_limit u64 \| args (u16 len)` (admin only) |
| 34 | SetContractAdmin | `contract [32] \| admin [32]` (admin only; zero = immutable for good) |

- `code_hash = blake3(code)`. `address = blake3("pearl-sol:contract:v1" || creator || nonce u64 LE)`,
  the Instantiate tx's sender and nonce; no ed25519 key controls it.
- Only DeployCode may exceed the 16 KiB tx cap (up to `MAX_DEPLOY_TX_SIZE` = 257 KiB); blocks,
  p2p messages (8 MiB) and RPC bodies (2 MiB) are unchanged.
- Validity: from the activation height (8); `gas_limit <= tx_gas_limit`; `gas_limit <=
  block_gas_limit - gas used so far in the block`; fee floor (5). A DeployCode is also invalid
  when its code fails validation (4) or is already deployed, so it is checked at admission.
- **Once fee and nonce are taken, Instantiate / Call / Migrate / SetContractAdmin always land.**
  A failure (revert, out of gas, trap, refused) undoes every state change except fee and
  nonce, and the receipt says why. This is what makes gas a real DoS cost: a miner is paid for
  running a failing call.

## 3. Contract model

- **Deploy**: code is uploaded once and shared by hash; the deployer locks a storage deposit
  for the bytes (`len x storage_byte_deposit`, held in the code record for good, counted in
  the supply invariant). Validation runs at deploy (4).
- **Instantiate**: creates the contract record, moves `value` CADRE to it, runs the
  `instantiate` entry point if exported. `admin` decides upgrades: zero = the contract can
  never change; otherwise only `admin` may Migrate (swap the code, keep storage and balance,
  run the new code's `migrate` entry) or SetContractAdmin. The explorer and `pearl contract
  show` print "immutable" or the admin, so users can see who can change what they call.
- **Call**: moves `value` to the contract, runs `call`. Every contract is also an ordinary
  account: it holds CADRE in its Account record and can receive plain Transfers (no code
  runs for those).
- **Cross-contract calls** are synchronous (`call` host function) with their own gas cap; the
  callee runs in an overlay savepoint, so a failing callee's writes, transfers, events and
  native calls are undone while the caller decides what to do. **No reentrancy**: a contract
  already on the call stack cannot be called again (the ink! default, decided in the host, not
  left to an SDK macro [R9]). Stack depth is capped by `max_call_depth` (default 8).
- **Events**: up to 4 topics of 32 bytes (topic 0 = `blake3("pearl-sol:event:v1" || name)` by
  the SDK convention) and up to 4096 bytes of data; up to 64 per tx. Kept in the tx's stored
  receipt, indexed per contract (`getContractEvents`), and committed per block in the contracts
  record's `events_root` (7). Events of a failed tx are dropped.
- **Queries**: `queryContract` runs the `query` entry against the tip state in query mode
  (writes, transfers, value calls, native calls and events abort), with at most 50M gas.
  `simulateCall` runs `call` as a given sender without a signature or fee and nothing kept,
  for gas estimation (it returns gas used, return data, events and the debug log).
- **Storage**: keys up to 128 bytes, values up to 64 KiB, in the chain's QMDB state under
  `["cstore", contract || key]` (tag 46). Each item is charged `key + value + 64` bytes of
  deposit, paid by the tx sender when storage grows and refunded pro rata to the sender who
  shrinks it (the pallet-contracts rule [R5]). A sender who cannot cover the deposit gets a
  Refused receipt and nothing is written.

State records (schema tags 40-48, `chain/state/src/contracts.rs`): CodeInfo, code bytes, code
index, contract counters, Contract (code hash, admin, creator, label, heights, storage bytes
and items, deposit, calls, events), contract index, storage items, stored receipts with their
events, per-contract event index.

## 4. ABI and host functions

Contracts are wasm modules that export `memory` and any of `instantiate`, `call`, `query`,
`migrate` (no params, no results). Input, output and errors go through host functions.

**Input encoding** (SDK `abi`, CLI `TYPE:VALUE` args): `call` and `query` input is `method
(u8 len, UTF-8) | fields`; `instantiate` and `migrate` input is the fields alone. Fields: u8,
u16, u32, u64 (LE), bool (u8 0/1), addr / hash (32 bytes), bytes / str (u32 len + bytes).
Return data is fields without a method.

**Deploy-time validation** (consensus, `Vm::validate`): wasm 1.0 binary; no `start` section;
no floats anywhere (wasmi `floats(false)`); no SIMD, memory64, multi-memory, custom page sizes;
`EnforcedLimits::strict()` (10,000 functions, 32 params, 1,000 globals/segments, average
function body size); every import is a function from module `cadre` in the table below with
the exact signature; memory exported as `memory`, starting at <= 64 pages; at least one entry
point; entry points take and return nothing; size <= `max_code_size`. Compilation is eager.

**Run-time limits**: 4 MiB memory per frame (`memory.grow` past it returns -1), wasmi
recursion depth 1,000 and value stack 1,000,000 set explicitly (so a wasmi default change
cannot fork the chain), 64 events per tx, every trap mapped to a fixed message so receipts
are byte-identical on every node.

| Host function (module `cadre`) | Gas (5) | Effect |
|---|---|---|
| `input_len() -> i32`, `input_read(ptr)` | base + 1/byte | the call input |
| `env_read(ptr)` | base + 148 | `chain_id u32, height u64, self, caller, origin, value u64, miner` |
| `set_return(ptr, len)` | base + 1/byte | return data (<= 4096 bytes) |
| `revert(ptr, len)` | base | stop, undo, fail with the message |
| `scratch_len() -> i32`, `scratch_read(ptr)` | base + 1/byte | result buffer of the last read or call |
| `storage_read(k, klen) -> i32` | 10,000 + 2/byte read | value to scratch, length or -1 |
| `storage_write(k, klen, v, vlen)` | 20,000 + 20/byte | write (deposit settled at the end of the tx) |
| `storage_remove(k, klen)` | 20,000 + 20/byte of key | remove |
| `emit(topics, n, data, len)` | 5,000 + 20/byte | event |
| `balance(addr) -> i64` | 20,000 | CADRE balance of any account |
| `transfer(addr, amount) -> i32` | 60,000 | send from this contract (0 ok, 1 insufficient, 2 reserved account) |
| `call(addr, value, input, len, gas) -> i32` | 50,000 + 1/byte + the callee's gas | cross-contract call (0 ok, 1 failed); output or error to scratch |
| `native_read(kind, id, len) -> i32` | 30,000 | native record (6) to scratch |
| `native_call(kind, body, len) -> i32` | 200,000 + 50/byte | native tx body as this contract (6) |
| `blake3`, `sha256(ptr, len, out)` | 1,000 + 4/byte | hashes |
| `ed25519_verify(pk, msg, len, sig) -> i32` | 150,000 + 4/byte | raw ed25519 check (Cadre and Solana keys) |
| `gas_left() -> i64` | base | |
| `log(ptr, len)` | 1,000 + 1/byte | debug line, kept only by simulations |

base = 200. Loading a frame costs 20 gas per byte of the contract's code (whether or not the
node had it compiled), DeployCode 40 gas per byte.

How the host runs wasm against async state: every state-touching host function returns a
`Request` as a wasmi host error; wasmi keeps the paused call; the executor (an explicit frame
stack in `chain/executor/src/contracts.rs`, no async recursion) serves the request from its
overlay and resumes the call with the answer. Frames are isolated stores; each nested call
has its own overlay savepoint.

**SDK**: `crates/cadre-contract-sdk` (no_std, bump allocator, panic = revert): `dispatch`,
`init`, `env`, `storage`, `emit`, `transfer`, `balance`, `call`, `native::{job, account,
profile, follows, post, follow, call_raw}`, `abi::{Reader, Writer}`, hashes, ed25519. Examples
in `crates/cadre-contract-sdk/examples`: counter, escrow (CADRE escrow with payer, payee,
arbiter, deadline), token (balances, allowances, optional minter), bounty (agents submit work,
a judge awards it; or it is linked to a native job and pays whoever delivered it once its
evaluators complete it; optional town announcement), testkit (VM tests only).

**CLI**: `pearl contract build | deploy | instantiate | new | call | query | show | ls |
events | receipt | migrate | admin` (docs/CLI.md). `call` simulates first and sets the gas
limit to the simulated gas plus 25%.

## 5. Gas and fees

- 1 gas = 1 wasmi fuel unit. wasmi 1.1 charges 1 fuel for every operator (0 for nop, drop,
  block, loop and a few others) and charges bulk memory by bytes [R1]. Host costs are ours (4).
- **Fee floor** of a contract tx of `len` encoded bytes:
  `max(min_fee, contract_base_fee + contract_byte_fee x len + ceil(gas_limit x gas_price / 1,000,000))`.
  The fee goes to the block's miner like any fee; the unused gas
  `floor((gas_limit - gas_used) x gas_price / 1,000,000)` goes back to the sender, so whatever a
  sender pays above the floor stays with the miner as a tip.
- **Limits**: per tx `tx_gas_limit`, per block `block_gas_limit` (a tx whose limit does not
  fit the block's remaining gas is invalid, so a block never runs more than that).
- **Storage**: refundable deposit `storage_byte_deposit` per byte (3).
- Every number is an AdminConfig key (owner rule: every fee is editable on chain), valid from
  the activation height. Setting `block_gas_limit` to 0 stops every contract tx at once (an
  emergency brake that needs no code).

| key | name | default | why this default |
|---|---|---|---|
| 27 | gas_price | 10,000 base units per 1M gas | a counter call (150,670 gas) pays 1,507 base units for gas |
| 28 | block_gas_limit | 150,000,000 | about 1.1 s of the worst measured wasm, 0.13 s of typical code (11) |
| 29 | tx_gas_limit | 50,000,000 | a third of a block; the largest example op uses 676,280 gas |
| 30 | contract_base_fee | 1,000 | equal to `min_fee` |
| 31 | contract_byte_fee | 10 per tx byte | prices DeployCode by size: counter code (5,503 bytes) pays about 55,000 |
| 32 | storage_byte_deposit | 100,000 base units per byte | 0.0001 CADRE per byte, 0.1 CADRE per KiB held, refundable |
| 33 | max_code_size | 131,072 bytes | the four examples are 5.5-16.9 KB |
| 34 | max_call_depth | 8 | |

Measured gas of the examples under these defaults (`cargo test -p executor --test contracts
gas_table -- --nocapture`; fee = what was charged after the refund, base units):

| Operation | Gas | Fee |
|---|---|---|
| deploy counter (5,503 bytes) | 220,120 | 59,532 |
| deploy escrow (9,176 bytes) | 367,040 | 97,731 |
| deploy token (10,529 bytes) | 421,160 | 111,802 |
| deploy bounty (16,907 bytes) | 676,280 | 178,133 |
| instantiate counter | 153,328 | 4,664 |
| counter increment | 150,670 | 4,367 |
| instantiate token | 344,844 | 6,739 |
| token transfer | 285,934 | 6,030 |
| escrow open | 249,617 | 5,947 |
| escrow release | 290,180 | 5,742 |
| bounty post + town post (native call) | 616,830 | 9,549 |

(The fee column excludes storage deposits.) These gas figures are from the defaults above at
`gas_price` 10,000; deploy fees are mostly the byte fee.

## 6. Native access

Reads (`native_read(kind, id)`, ABI in `executor/src/contracts.rs::native_read`):

| kind | id | returns |
|---|---|---|
| 0 account | address | `balance u64, nonce u64` |
| 1 job | job id | `status u8, poster, deliverer, worker_pay u64, protocol_fee u64, deadline u64, created u64, closed u64, result_hash, evaluator count u8, threshold u8` |
| 2 agent | address | `registered_height u64, name (u8 len), uri (u16 len)` |
| 3 profile | address | `flags u16, operator [32], created_height u64, handle (u8 len), name (u8 len)` |
| 4 follow | follower or target | `height u64` |
| 5 contract | address | `code_hash, admin, creator, created_height u64` |

Calls (`native_call(kind, body)`): the contract sends a native tx body **as itself**: it is
the sender, it pays from its own balance, its account nonce numbers the job and post ids, and
the body runs through the same code as a signed tx (`exec_body`). Allowed kinds: Transfer,
RegisterAgent, PostJob, SubmitResult, CompleteJob, ExpireJob, BridgeLock, SetProfile, Post,
React, Follow. Not allowed: AdminConfig, BridgeUnlock, SetRoute, ClearRoute, RegisterModel,
PostSegments, RejectResult, OperatorLink and the contract kinds. **Permission rule: a contract
can do what an account can do with its own key for those kinds, and nothing on behalf of its
caller**: no host function takes another account's authority, and a caller's CADRE reaches a
contract only as `value` the caller signed. A native call that fails returns status 1 with the
reason and undoes only itself; the contract decides whether to revert.

**Paying the mining pool (2026-10-22).** The services market asked for a way to send its fee to
the mining pool, so it funds miners like a job's protocol fee. The `transfer` host function
refuses reserved accounts (escrow and pool, status 2), but a native `Transfer` sent by the
contract as itself already credits the pool: `exec_body` refuses only the escrow as a Transfer
target, and the pool is an ordinary account record. So no new host function, activation height or
node rollout was needed: the capability is the existing native-call rule, now named and pinned.

- SDK: `native::mining_pool()` (`blake3("pearl-sol:mining-pool:v1")`, computed in the contract,
  checked against the chain's `pool_address()` in the tests) and `native::pay_mining_pool(amount)`
  (a Transfer body `pool [32] | amount u64` through `native_call`). Measured: 328,019 gas for a
  probe call that pays the pool (fee 6,131 base units); in the market, a completing verdict that
  pays the fee to the pool costs 1,005,503 gas against 922,182 for a plain fee_to (fee 14,106
  against 13,272 base units).
- Pay in only, never out. Every way a contract could move CADRE is checked against the pool in
  `contracts/tests/tests/pool.rs` (probe contract `contracts/poolprobe`, test-only, never
  deployed): `transfer` to the pool or the escrow is refused; a native Transfer moves only the
  contract's own balance (asking for more than it holds fails and the pool is untouched); a native
  Transfer to the escrow is refused; the pool is not a contract (calls to it fail); a tx signed
  from the pool address is refused (reserved address cannot send); AdminConfig, BridgeUnlock and
  the other kinds a contract may not run natively stay refused. The pool's only debits remain the
  executor's own: block subsidy (emission) and the refund of a job's unpaid protocol fee to its
  poster.
- Supply invariant: checked after every block in those tests; a pool credit on a branch that a
  reorg drops is undone with the branch's write set, and the same verdict on the winning branch
  pays once (`pool_credit_on_a_dropped_branch_is_undone`). The chain's multi-node reorg test for
  contract calls (`chain/bin/agent-node/tests/contracts_rpc.rs`) covers the node path.

## 7. State commitment and proofs (Solana light client)

The 256-byte header is unchanged (the deployed Solana light client parses it). Two
commitments cover contracts:

1. **Contracts record, a tx_root leaf.** Every block with at least one contract tx carries an
   89-byte record (version 0x02: `height, receipt_count, receipts_root, event_count,
   events_root, gas_used`), placed in the tx_root leaves right before the coinbase receipt
   (which stays last). `receipts_root` and `events_root` are Merkle roots (the tx_root rule)
   over the block's receipts and events in order. It travels in the block body after the
   coinbase (`0x01 || record`); a block without contract txs encodes exactly as before. After
   execution the node requires the body's record to equal the one execution produced
   ("contracts record mismatch" rejects the block). A record never parses as a tx (0x01) or a
   coinbase receipt (0x00, 97 bytes). So Solana proves a contract event, or that a call
   succeeded, with two inclusion paths under a final header and no state proof:
   `pearl_verify_sbf::contracts::verify_event(header, record, record_path, event, event_path)`
   and `verify_receipt(...)`. getBlock returns the record and its path; the event path comes
   from the block's events (one event: empty path).
2. **Contract storage under `state_root`.** Storage items live in the chain's QMDB `current`
   database, whose root is the header's `state_root`; `getContractStateProof {address, key}`
   returns a current-value proof (229 bytes for one item in the test) against the FINAL header,
   checked by `Store::verify_state_proof`. A Solana-side verifier for QMDB proofs (SHA-256 MMR
   plus the activity bitmap chunk) is not built yet (12); events are the path that works on
   Solana today, which is how bridges read EVM chains too (receipts and logs).

header-relay keeps the record leaf when it builds tx_root paths (`Node::block_body` appends it
after the txs), so job registration keeps working in blocks that also carry contract txs. That
change must be running on the relays before any block mixes the two (8).

## 8. Activation and rollout

- `agent_types::contracts::contracts_upgrade_height(chain_id)`: Cadre public devnet (chain id
  1128350802) at `DEVNET_CONTRACTS_HEIGHT` = 4700, every other chain from genesis. The height
  is inert until the first contract tx lands: a block without contract txs carries no record and
  leaves the config record as it was, so a node that has not upgraded keeps agreeing until a
  contract tx is mined. No contract tx is sent on devnet before all three nodes run the upgrade.
  Live: all three devnet nodes upgraded 19:32-19:40 UTC 2026-10-21; the first contract tx landed in
  block 4703; the example contracts and their addresses are in docs/DEVNET.md "Contracts". Below it the
  executor refuses the contract kinds and config keys 27-34 and the node RPC refuses them at
  admission, so an upgraded node agrees with an old one on every block below the height (an old
  node fails a contract tx at decode, a new one at execution). From the height every node must
  run the upgrade; a node without it rejects the first block carrying a contract tx (or a
  contracts record) and forks off.
- The config record keeps its encoding (19, or 27 values with the social keys) until an admin
  sets a contract key, so state roots match old nodes below the height. `state_root` is
  otherwise unchanged by the upgrade.
- Rollout: one rolling upgrade of cadre-node and cadre-miner, box 1, 2, 0, with tips, finalized
  hash and peers equal between boxes (docs/DEVNET.md); header-relay with the record-leaf change
  on boxes 1, 2 and cadre-settle on box 0 before the first contract tx.

## 9. Security

| Threat | Mitigation |
|---|---|
| VM sandbox escape (memory safety bug in the VM) | Interpreter only (no JIT, no native code from user input); pure-Rust wasmi past both public advisories; memory capped at 4 MiB per frame (the advisory workaround for CVE-2025-66627 [R1]); small fixed import table; frames are separate wasmi stores |
| Nondeterminism | No floats, no SIMD, no threads, no clock, no randomness, no host environment; eager compile; explicit stack limits; fixed trap messages; exact wasmi pin; the same state digest on arm64 and x86_64 (11) |
| Gas DoS | Every op and host call costs gas; failing calls still pay; per-tx and per-block limits sized for the measured worst case; DeployCode validation charged per byte and done at admission; module load charged per byte regardless of the cache; query and simulation capped at 50M gas on the RPC |
| Compile bombs | `EnforcedLimits::strict()`, code size cap, linear-time translation, deploy charged by size (CosmWasm CWA-2023-004 and NEAR's 7.6 s Cranelift compiles were JIT/compiler problems [R2, R3]) |
| Reentrancy | Refused by the host for every contract (no opt-in yet); synchronous calls with savepoints; Curve 2023 was a compiler-level lock bug, so the rule is not left to the SDK [R9] |
| State bloat | Refundable per-byte storage deposit paid by the sender who grows storage; code deposits locked for good |
| Upgrade keys | Admin chosen at instantiate, zero = immutable; SetContractAdmin to zero freezes for good; explorer and CLI show it; migrations keep storage, so users should prefer immutable contracts or admins that are themselves contracts or multisig keys |
| Contracts acting for users | Native calls run as the contract only; value moves only as signed `value`; no `tx.origin`-style authority in native calls |
| Chain admin key | The chain's AdminConfig key can change gas parameters (including the 0 brake). That is the chain's existing trust model (SPEC 2), not new |

**Outside audit before mainnet** (required, not done): the host (`chain/contracts`), the driver
(`chain/executor/src/contracts.rs`), the gas schedule, and the light-client proof path. Firms
with WASM VM host experience [R12]: SRLabs and Runtime Verification (both audited wasmi), Oak
Security (CosmWasm chains), Trail of Bits (Arbitrum Stylus). A fuzzing campaign over the host
ABI (random modules and host-call sequences, comparing two nodes' results) should run before
the audit.

## 10. Speed: faster blocks later

Contracts do not block a move to shorter blocks or a finality layer:
- execution is bounded by `block_gas_limit` (about 1.1 s worst case today), an AdminConfig key,
  so a shorter block target can lower it in the same change;
- receipts and events are committed per block (the record), and nothing in the contract model
  depends on the 60 s target, K, or wall time (contracts see `height`, not timestamps);
- a finality gadget only changes when a block counts as final: queries already read the tip,
  proofs already use final headers.

Options, with their tradeoffs for useful PoW (sources in [R10]):

| Option | What it gives | Cost and risk for Cadre |
|---|---|---|
| Shorter PoW target (e.g. 15 s) | 4x more blocks, 4x sooner first confirmation | Orphans grow with propagation time over block time: with the 6.5 s median propagation Decker and Wattenhofer measured on Bitcoin, the first-order estimate is about 11% at 60 s already (our arithmetic [R10]); Ethereum PoW at about 13-15 s ran 7.41% uncles in its first 280k blocks. A Pearl proof is about 25 KB and verification takes real CPU, so propagation is slower than a bare header; job blocks also need A/B data available. Needs GHOST-style uncle accounting or more orphans waste useful work. The light client relays 4x the headers (SPEC 5.3: about 12 txs per header) |
| Pre-confirmations (weak blocks / sub-blocks, Bobtail) | Sub-second to few-second soft confirmations without changing the main PoW target (Ergo proposed sub-blocks at 1/128 difficulty, about one per second; Bobtail reduces block-time variance as 1/k) | Weak blocks are a second message type and a selfish-mining surface; with useful PoW, a weak block could carry a lower-difficulty Pearl proof, so miners keep doing the same matmul. Contracts would show "pre-confirmed" receipts that can still reorg |
| Finality layer (a committee or stake signing checkpoints over PoW: Casper FFG over PoW, Hybrid Consensus) | Fast, irreversible finality (seconds) independent of K = 30 blocks | Adds a validator set and slashing: a new trust assumption next to PoW; the Solana light client would check committee signatures as well as work. Ethereum's PoW + FFG hybrid was abandoned for full PoS |
| BlockDAG (GHOSTDAG / DAGKNIGHT, Kaspa: 10 blocks/s since 2025) | High block rate without orphan loss | A redesign of fork choice and of contract execution order (parallel blocks conflict); largest change |

Recommended path when speed is wanted: (1) pre-confirmations by weak blocks carrying
lower-target Pearl proofs (no change to finality or the light client), then (2) a moderately
shorter target (30 s) with uncle-style credit for orphaned useful work, measured on the devnet
first. A finality committee only if fast irreversible finality becomes a product need.

## 11. Tests and measurements

- `chain/contracts/tests/vm.rs`: the examples validate; deterministic gas; query mode cannot
  write; 11 malicious modules refused at deploy (floats in code and in signatures, unknown and
  foreign imports, wrong signatures, imported memory, start function, no memory, no entry,
  entry with params, 65-page memory, size limit); runtime attacks stop deterministically
  (infinite loop out of gas with all gas used, unbounded recursion = stack overflow trap,
  memory.grow past the cap returns -1, out-of-bounds host pointers, oversized keys, a host
  call without gas, over 64 events).
- `chain/executor/tests/contracts.rs`: end to end with fees, refunds and receipts; failures land
  with state undone (revert, out of gas, value calls, calls to nothing); cross-contract calls,
  reentrancy refused (direct and through another contract), a failing callee undone while the
  caller continues, the call depth limit, value through nested calls; storage deposits and
  refunds, a sender who cannot pay; escrow, token and bounty flows including a native job
  linked to a bounty and a town post by a contract; block gas limit and fee floor; migrate and
  admin rules; devnet activation height and the config encoding; a determinism digest; the gas
  table. The supply invariant is checked after every block.
- `contracts/tests/tests/pool.rs` (2026-10-22, 6 tests): paying the mining pool from a contract
  (6): the SDK pool address is the chain's, the market fee lands in the pool, fee_to switches to the
  pool and back (never the escrow), the deployed devnet v1 market (fixture, code hash 1647ce24...)
  migrates with an open hire intact, no contract path debits the pool, a dropped branch undoes the
  credit; supply invariant every block. `contracts/tests/tests/halt.rs` (3): the protocol halt
  record (authority only, scope bits, immutable code, status layout).
- `chain/state/tests/contract_proof.rs`: a contract storage item proves against the committed
  state_root; wrong value, key or root fail; an old proof fails after an overwrite.
- `chain/bin/agent-node/tests/contracts_rpc.rs`: three node actors; deploy, instantiate and
  calls sent to different nodes and mined by different nodes; every node returns the same
  contract, code, receipts, query results, events and block contracts record; a call on a
  losing branch is undone by the reorg; a final storage proof verifies.
- `crates/pearl-verify-sbf/tests/contracts.rs`: the record, an event and a receipt prove under
  a header's tx_root from the published vectors; tampering fails.
- **Determinism across machines**: `CADRE_DIGEST=1 cargo test -p executor --test contracts
  determinism_digest -- --nocapture` (a fixed scenario of deploys, instantiates, calls, an
  out-of-gas call and a nested call, digest over the whole state) printed
  `6b70aa8961335901a420fc65ed795062a0fbe2719ab64100d2f725accb31b435` on the Mac (arm64) and on
  the test box (x86_64), 2026-10-21. (An earlier build with other defaults gave `3460c5af...` on
  both.) The multi-node test checks state roots through consensus: a node rejects any block
  whose state_root differs from its own execution.
- **Measured** (test box, 8 vCPU x86_64, release, 2026-10-21; `cargo test --release -p contracts
  --test vm -- --ignored --nocapture measure` and `-p state --test contract_proof -- --ignored`):

| Workload | ns per gas |
|---|---|
| arithmetic loop (example `burn`) | 0.77-0.85 |
| tight `br` loop | 2.25-2.28 |
| `i64.div_u` loop | 3.14 |
| `memory.fill` 4 MiB loop | 2.31 |
| `memory.copy` 2 MiB loop | 4.88 |
| `call_indirect` loop (worst found) | 7.36 |
| `gas_left` host call loop | 0.30 |
| `blake3` 4 KiB host call loop | 0.09 |

Module validation and compile: 26.6-29.8 ns per code byte; instantiating a cached module 4.8-4.9
ns per byte; merkleizing a 64-byte state write about 3.0 us (10,000 writes, three rounds). The
box was shared with other jobs (load average about 29 during the first run, about 5 during the
second), so these are upper-side figures.

## 12. Open items

- Per-operator fuel costs: wasmi 1.1 charges every operator 1, which makes `call_indirect`
  about 10x more expensive per gas than arithmetic; moving to wasmi 2.0's `OperatorCost` table
  would let the block gas limit rise for typical code. Consensus change, behind a height.
- A Solana verifier for QMDB current-value proofs (storage proofs on Solana).
- Reentrancy opt-in for contracts that want callbacks.
- Reproducible contract builds (`pearl contract build` with path remapping) so explorers can
  show verified source.
- Receipt and event pruning (they live in state for good today; gas covers their size).
- An EVM next to WASM, if Ethereum tooling is wanted (1).
- Outside audit and a host-ABI fuzzing campaign before mainnet (9).
- If the audit prefers one rule for both paths, the `transfer` host function could accept the pool
  as a target (pay in only) behind a new activation height. Not needed today: native Transfer is
  the path (6, "Paying the mining pool").
