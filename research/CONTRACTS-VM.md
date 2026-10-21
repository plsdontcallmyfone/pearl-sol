# Cadre contracts VM research (2026-10-21)

Evidence for the design doc's comparison and security sections. Working choice:
WASM on `wasmi` with fuel metering and a Rust SDK. Every figure below carries the
URL it came from; where something was searched for and not found, it says so.
Versions are as of 2026-10-21 (crates.io API, `https://crates.io/api/v1/crates/<name>`).

---

## 1. wasmi

**Version**
- Current stable: `wasmi 2.0.0`, published 2026-09-01 (crates.io `max_stable_version` = `newest_version` = 2.0.0). https://crates.io/api/v1/crates/wasmi
- GitHub release v2.0.0 dated 2026-09-01, preceded by 2.0.0-beta.9 (2026-07-29) and beta.10 (2026-08-10). https://github.com/wasmi-labs/wasmi/releases
- README design goals: "simple, correct and deterministic execution", no_std/embedded use, and a translation step described as resistant to JIT/compiler bombs. https://github.com/wasmi-labs/wasmi

**Fuel metering API (2.0.0)**
- `Config::consume_fuel(bool)`: "Configures whether Wasmi will consume fuel during execution to either halt execution as desired." Disabled by default. Exhaustion raises `TrapCode::OutOfFuel`. https://docs.rs/wasmi/latest/wasmi/struct.Config.html
- `Store::set_fuel` / `Store::get_fuel`: both return an error "if fuel metering is disabled" (must be enabled via `Config::consume_fuel`). https://docs.rs/wasmi/latest/wasmi/struct.Store.html
- `Config::operator_cost(OperatorCost)`: "Configures the fuel cost of each WebAssembly operator." `OperatorCost` is a struct with one `u8` field per Wasm operator. https://docs.rs/wasmi/latest/wasmi/struct.OperatorCost.html
- `Config::fuel_cost(CustomFuelCosts)`: dynamic costs for lazy translation/validation and for byte copies in `memory.{grow,copy,fill,init}` and `table.{grow,copy,fill,init}`. Fields: `bytes_copied_per_fuel`, `fuel_per_bytes_translated`, `fuel_per_bytes_validated`. https://docs.rs/wasmi/latest/wasmi/struct.CustomFuelCosts.html
- Default costs (source, tag v2.0.0): `nop`, `drop`, `block`, `loop`, `unreachable`, `return`, `else`, `end` cost 0; "Everything else, just call it one operation" = 1 fuel. https://github.com/wasmi-labs/wasmi/blob/v2.0.0/crates/wasmi/src/engine/costs.rs
- Default dynamic costs: 64 bytes copied per fuel, 7 fuel per byte translated, 2 fuel per byte validated. Byte-copy fuel also applies to call parameter passing and `copy_span`/`return_many` style IR ops. https://github.com/wasmi-labs/wasmi/blob/v2.0.0/crates/core/src/fuel.rs
- Implication: the default table is flat (1 per op, so `i64.div` = `i32.add`). Cadre should supply its own `OperatorCost` table benchmarked on reference hardware and pin it in consensus params.

**Resumable calls**
- `ResumableCall` variants: `Finished`, `HostTrap(ResumableCallHostTrap)` ("can be resumed"), `OutOfFuel(ResumableCallOutOfFuel)` ("ran out of fuel but can be resumed"). Typed equivalents: `TypedResumableCall`, `TypedResumableCallHostTrap`, `TypedResumableCallOutOfFuel`. https://docs.rs/wasmi/latest/wasmi/enum.ResumableCall.html , https://docs.rs/wasmi/latest/wasmi/index.html
- `ResumableCallOutOfFuel::required_fuel() -> u64` ("minimum required fuel to progress execution") and `resume(ctx, outputs)`. So out-of-fuel is resumable in 2.0.0 after topping up with `set_fuel`. https://docs.rs/wasmi/latest/wasmi/struct.ResumableCallOutOfFuel.html
- Use for Cadre: HostTrap resumption is the hook for async host calls (e.g. a QMDB read that is not cached) without unwinding the guest. For consensus, out-of-fuel should be a hard abort, not a resume across blocks.

**Proposal toggles (Config, 2.0.0 defaults)**
- `floats(bool)`: "Enable or disable Wasm floating point (f32 and f64) instructions and types." Enabled by default; needs the `validate` crate feature. https://docs.rs/wasmi/latest/wasmi/struct.Config.html
- Enabled by default: `wasm_bulk_memory`, `wasm_multi_value`, `wasm_reference_types`, `wasm_tail_call`, `wasm_multi_memory`, `wasm_extended_const`, `wasm_mutable_global`, `wasm_sign_extension`, `wasm_saturating_float_to_int`, `wasm_simd` and `wasm_relaxed_simd` (both behind the `simd` crate feature), `wasm_memory64` (behind `memory64` feature). Disabled by default: `wasm_custom_page_sizes`, `wasm_wide_arithmetic`. Same URL.
- Threads, GC and exception-handling are not supported (README marks them as tracking issues). https://github.com/wasmi-labs/wasmi/blob/v2.0.0/README.md
- Implication: relaxed-simd is non-deterministic by spec and is ON by default when the `simd` feature is compiled in. Cadre should build without `simd` or explicitly disable it, and turn off floats, memory64 and multi-memory.

**Compilation mode**
- `CompilationMode`: `Eager`, `LazyTranslation` (default: validate eagerly, translate on first use), `Lazy` (validate and translate lazily). `Lazy` "must not be used if the result of Wasm execution must be deterministic amongst multiple Wasm implementations." https://docs.rs/wasmi/latest/wasmi/enum.CompilationMode.html

**Stack and recursion limits**
- `Config::set_max_recursion_depth`, `set_max_stack_height` (bytes, traps when exceeded), `set_min_stack_height`, `set_max_cached_stacks`. https://docs.rs/wasmi/latest/wasmi/struct.Config.html
- Defaults (source): max recursion depth 1000, min stack height 1,000, max stack height 1,000,000, cached stacks 2. https://github.com/wasmi-labs/wasmi/blob/v2.0.0/crates/wasmi/src/engine/limits/stack.rs

**Parse/compile limits (anti compile-bomb)**
- `Config::enforced_limits(EnforcedLimits)`; "By default no limits are enforced." https://docs.rs/wasmi/latest/wasmi/struct.EnforcedLimits.html
- `EnforcedLimits::strict()` values: max_globals 1000, max_functions 10,000, max_tables 100, max_element_segments 1000, max_memories 1, max_data_segments 1000, max_params 32, max_results 32, min average 40 bytes per function once function bodies total at least 1000 bytes. https://github.com/wasmi-labs/wasmi/blob/v2.0.0/crates/wasmi/src/engine/limits/engine.rs

**Memory limits (StoreLimits)**
- `StoreLimitsBuilder`: `memory_size` (unlimited by default), `table_elements` (unlimited), `instances`/`tables`/`memories` (10,000 each per Store), `trap_on_grow_failure` (default false, so `memory.grow` returns -1). Registered via `Store::limiter`. https://docs.rs/wasmi/latest/wasmi/struct.StoreLimitsBuilder.html

**Who uses wasmi**
- README user list: Soroban, Typst, Zellij, Josh, Ripple, Firefly Zero, icu4x, orbitinghail, smoldot, Munal OS, Ayaka, Project Oak. https://github.com/wasmi-labs/wasmi
- Stellar Soroban chose an interpreter (wasmi) over a JIT for security: a JIT is a larger codebase emitting native code, so its bugs tend to be RCE-class, and compilation itself can be a "JIT bomb". https://stellar.org/blog/developers/why-doesnt-soroban-use-a-jit
- Substrate `pallet-contracts` 50.0.0 depends on `wasmi ^0.32.3`; "Gas metering of Wasm executed instructions is being done on the engine side." https://docs.rs/pallet-contracts/latest/pallet_contracts/
- Parity forked `pallet-contracts` into `pallet-revive`, which uses "PolkaVM instead of wasmi". https://gitlab.parity.io/parity/mirrors/polkadot-sdk/-/commit/559fa1db0594a81d5dbf343613ba2f3fc16708da (see section 5)

**Security advisories**
- GHSA-g4v2-cjqp-rfmq / CVE-2025-66627 (high, published 2025-12-08): use-after-free in linear memory under certain growth conditions. Affected 0.41.0-0.41.1, 0.42.0-0.47.1 (sources disagree whether 0.47.1 or 0.47.2 is the fix), 0.50.0-0.51.2, 1.0.0; fixed 0.41.2, 0.51.3, 1.0.1. Workaround: cap max linear memory. https://github.com/wasmi-labs/wasmi/security/advisories/GHSA-g4v2-cjqp-rfmq , https://nvd.nist.gov/vuln/detail/CVE-2025-66627
- GHSA-75jp-vq8x-h4cq / CVE-2024-28123 (critical, 2024-03-07): out-of-bounds write when the host calls or resumes a Wasm function with more than 128 parameters; fixed in 0.31.1. https://nvd.nist.gov/vuln/detail/CVE-2024-28123
- These are the only two GHSA advisories listed on the repo (GitHub advisories API, 2026-10-21). No RUSTSEC entry specific to wasmi was found in search.

**Audits**
- Runtime Verification Inc. audited wasmi 0.36.0 to 0.38.0 for the Stellar Development Foundation. https://github.com/wasmi-labs/wasmi/blob/main/resources/audit-2024-11-27.pdf
- SRLabs audited wasmi 0.31.0 for Parity Technologies. https://github.com/wasmi-labs/wasmi/blob/main/resources/audit-2023-12-20.pdf
- No audit of the 2.x line was found.

**Speed reference point**
- On a NES-emulator benchmark (ms per frame, lower is better): wasmi 108, wasmer singlepass 10.8, wasmer cranelift 4.8, wasmtime 5.3, solana_rbpf interpreted 6930, solana_rbpf JIT about 625, PolkaVM 0.1 10.7. This is a 2023 measurement on old wasmi; 2.x has a new engine and was not benchmarked here. https://forum.polkadot.network/t/announcing-polkavm-a-new-risc-v-based-vm-for-smart-contracts-and-possibly-more/3811

---

## 2. wasmtime

- Current stable: `wasmtime 50.0.0` (crates.io, updated 2026-10-20). https://crates.io/api/v1/crates/wasmtime
- Determinism guide: enable `Config::cranelift_nan_canonicalization` (adds float overhead), make relaxed SIMD deterministic with `Config::relaxed_simd_deterministic` or disable it, and "use deterministic fuel-based interruption rather than non-deterministic epoch-based interruption." Suggests limiting/pre-sizing memories so `memory.grow` outcomes do not depend on host state. https://docs.wasmtime.dev/examples-deterministic-wasm-execution.html
- `epoch_interruption`: faster than fuel (docs cite up to 2-3x in some measurements) but "The interruptions are not deterministic." `cranelift_nan_canonicalization` defaults to false. https://docs.rs/wasmtime/latest/wasmtime/struct.Config.html
- Advisory history (GitHub advisories API, 2026-10-21), notable entries:
  - GHSA-ff4p-7xrq-q5r8 / CVE-2023-26489 (critical, 2023-03-08): guest-controlled out-of-bounds read/write on x86_64 (Cranelift codegen). https://github.com/bytecodealliance/wasmtime/security/advisories/GHSA-ff4p-7xrq-q5r8
  - CVE-2023-27477 (2023-03-08) and CVE-2023-41880 (2023-09-14): SIMD miscompilations on x86_64 (low). https://github.com/bytecodealliance/wasmtime/security/advisories/GHSA-gw5p-q8mj-p7gh
  - 2024: CVE-2024-30266 (externref panic, low), CVE-2024-47763 (tail calls plus stack traces crash, medium), CVE-2024-47813 (race leading to CFI/type-safety violations, low), CVE-2024-51745 (Windows device filenames, low). No critical sandbox escape in 2024. https://github.com/bytecodealliance/wasmtime/security/advisories
  - 2026-04-09: CVE-2026-34971 (critical, aarch64 Cranelift miscompiled heap access, sandbox escape) and CVE-2026-34987 (critical, Winch backend sandbox-escaping memory access). https://github.com/bytecodealliance/wasmtime/security/advisories/GHSA-jhxm-h53p-jm7w , https://bytecodealliance.org/articles/wasmtime-security-advisories
  - 2026-09-24: GHSA-m63x-6p34-q65x, "`call_ref` and exception `catch` can drop some fuel accounting, leading to exponential fuel amplification" (medium): a metering bug, directly relevant to consensus use. https://github.com/bytecodealliance/wasmtime/security/advisories/GHSA-m63x-6p34-q65x
  - 2026-10-19: two more critical Cranelift miscompilations (GHSA-897f-g5gm-fmxm bit-packing overflow, GHSA-mghf-wjfp-p5xm aarch64 `br_table`). https://github.com/bytecodealliance/wasmtime/security/advisories
- Dependency footprint (2023 count): wasmtime 134 crates / 1,169,225 lines vs PolkaVM 6 crates / 15,222 lines (author notes much of wasmtime is generated bindings). https://forum.polkadot.network/t/announcing-polkavm-a-new-risc-v-based-vm-for-smart-contracts-and-possibly-more/3811
- Third-party audit of wasmtime: not found. The project relies on fuzzing and `cargo vet` dependency review. https://fitzgen.com/2022/09/13/security-and-correctness-in-wasmtime.html

**On-chain users and the compile-bomb lesson (NEAR, 2026)**
- NEAR replaced NearVM (its own single-pass fork of Wasmer) with Wasmtime in nearcore 2.12. https://blog.nearone.org/research/2026/07/01/replacing-nears-contract-vm-with-wasmtime.html , https://newreleases.io/project/github/near/nearcore/release/2.12.0
- NEAR rejected wasmtime fuel (no size-linear charging for bulk ops, no stack-height metering) and kept its own `finite-wasm` instrumentation; a pathological 996-function contract grew from about 14.7 KB to 124 KB (8.5x) from metering alone. Same blog URL.
- Compile times on high-end hardware: NearVM worst real contract about 100 ms, crafted about 9 ms; Cranelift worst real about 600 ms, crafted about 7.6 s (rejected); Winch (shipped) worst real about 300 ms, crafted about 36 ms. Crafted contracts reached about 500 MB artifacts, so a 16 MB `max_instrumented_code_size` was added. Bomb shapes: basic-block storms, dense `call_indirect`, NaN-canonicalization storms, long-lived locals, about 1000-parameter functions. Same blog URL.
- A wall-clock compile timeout was rejected because "consensus needs every validator to reach the same verdict" (paraphrase). Same blog URL.
- Execution: median per-call time about 2 ms (NearVM) to about 500 us (Wasmtime), gas differences under 0.002% in shadow comparison. Same blog URL.
- CosmWasm uses Wasmer (singlepass), not wasmtime (section 3). Arbitrum Stylus uses a vendored Wasmer (section 6).

---

## 3. CosmWasm

- Runtime: `cosmwasm-vm` pins `wasmer = "=5.0.6"` with the `singlepass` compiler ("layout of cached modules can change between patch versions"), and calls `canonicalize_nans(true)`. https://github.com/CosmWasm/cosmwasm/blob/main/packages/vm/Cargo.toml , https://github.com/CosmWasm/cosmwasm/blob/main/packages/vm/src/wasm_backend/engine.rs
- Gas target: 1 Teragas (10^12 CosmWasm gas) per second, "inspired by NEAR". Per-operation cost was 150 (pre 2.1), re-benchmarked to 170 in CosmWasm 2.1 using an Argon2 contract (35,138,122 ops in 6 ms on an i7-6700). https://github.com/CosmWasm/cosmwasm/blob/e50490c4199a234200a497219b27f071c3409f58/docs/GAS.md
- Multiplier: the GAS.md text says it "was measured and set to 100 a while ago". The current wasmd code has `DefaultGasMultiplier = 140_000` ("how many CosmWasm gas points = 1 Cosmos SDK gas point"), with a comment that it "deserves more reproducible benchmarking". https://github.com/CosmWasm/wasmd/blob/main/x/wasm/types/gas_register.go
- Other wasmd SDK-gas constants: instance load 60,000; compile 3 per byte; uncompress 0.15 per byte; pin/unpin 500,000. Same URL.
- Max code size: `MaxWasmSize` = 800 KiB; `MaxProposalWasmSize` = 3 MiB; salt max 64 bytes; label max 128. https://github.com/CosmWasm/wasmd/blob/main/x/wasm/types/validation.go
- Admin/migrate model: admin is optional and set at instantiation; without it the contract is immutable. The admin can migrate (`MsgMigrateContract`, which runs the new code's `migrate` entry point), transfer admin, or clear it to freeze the contract. A failing `migrate` reverts the whole transaction. https://docs.archway.io/developers/smart-contracts/migration , https://docs.cosmwasm.com/docs/smart-contracts/migration
- Actor model and reentrancy: contracts cannot call each other directly; they return messages that run after the caller finishes, depth-first, and a failure reverts the whole transaction; docs compare this to an automatic mutex over the contract. Submessages (since 0.14) let a contract receive the result via `reply`. https://docs.archway.io/developers/cosmwasm-documentation/architecture/comparison-with-ethereum , https://docs.cosmwasm.com/docs/architecture/actor
- Advisories (index https://github.com/CosmWasm/advisories/tree/main/CWAs, 34 CWAs from 2021 to 2026):
  - CWA-2023-002 "Stack overflow crash (Codename Cherry)": recursive guest/host loop (contract's `allocate` export calls an import that calls `allocate`) crashes the VM. https://github.com/CosmWasm/advisories/blob/main/CWAs/CWA-2023-002.md
  - CWA-2023-004 (High) "Excessive number of function parameters in compiled Wasm": a large signature referenced by many functions is inlined on compile, "pretty much a compression bomb", producing modules of "several hundreds of megabytes or even gigabytes" and node crashes. Fixed in wasmvm 1.2.6 / 1.3.1 / 1.4.2 / 1.5.1. https://github.com/CosmWasm/advisories/blob/main/CWAs/CWA-2023-004.md , write-up https://www.certik.com/resources/blog/risk-and-security-enhancement-for-app-chains-an-in-depth-writeup-of-cwa-2023
  - CWA-2024-002 (Medium): wrapping instead of panicking overflow in `Uint256/512::pow` and `Int256/512::neg` in cosmwasm-std. https://github.com/CosmWasm/advisories/blob/main/CWAs/CWA-2024-002.md
  - CWA-2024-007 (Medium) "Incorrect metering": gas tracking in Wasmer lets a contract use much more CPU than paid for, able to "temporarily DoS a chain by stalling block production". https://github.com/CosmWasm/advisories/blob/main/CWAs/CWA-2024-007.md
  - CWA-2025-001 (Medium): "Malicious smart contract can crash the chain"; reliably triggerable only with a malicious contract, so permissionless chains are more exposed. https://github.com/CosmWasm/advisories/blob/main/CWAs/CWA-2025-001.md
  - CWA-2026-001 (Medium): node memory saturation. https://github.com/CosmWasm/advisories/blob/main/CWAs/CWA-2026-001.md
- Audit: Oak Security audited CosmWasm and wasmvm (reports dated 2023-03-27) and wasmd (2023-10-17). https://github.com/oak-security/audit-reports/tree/main/CosmWasm
- Data point: Sei (which ran CosmWasm next to EVM) stopped new CosmWasm uploads/instantiations under Proposal 115. https://docs.sei.io/learn/dev-interoperability

---

## 4. NEAR

- Storage staking: 1E19 yoctoNEAR per byte, i.e. 100 KB per 1 NEAR. Accounts get 770 bytes free (covered by 0.007 NEAR at creation). https://docs.near.org/protocol/storage/storage-staking
- Runtime parameters (nearcore master): `wasm_regular_op_cost: 822_756` gas per regular Wasm op; `max_contract_size: 4_194_304`; `max_transaction_size: 1_572_864`; `storage_amount_per_byte: 10000000000000000000 yN`; `max_gas_burnt: 1_000_000_000_000_000` (1 Pgas); `max_stack_height: 262_144`; `max_memory_pages: 2_048`; `max_functions_number_per_contract: 10_000`; `max_promises_per_function_call_action: 1_024`; `wasm_contract_loading_bytes: 1_089_295`; `vm_kind: Wasmtime`. https://github.com/near/nearcore/blob/master/core/parameters/res/runtime_configs/parameters.yaml
- VM: NearVM (Wasmer fork) until nearcore 2.12, now Wasmtime with Winch plus `finite-wasm` instrumentation (section 2). https://blog.nearone.org/research/2026/07/01/replacing-nears-contract-vm-with-wasmtime.html
- Async promises: the caller and callback run in different blocks, typically 1-2 blocks apart; the contract can receive other calls in between. If the callee fails there is "no automatic rollback"; the callback must check and undo manually; callbacks should be marked private. https://docs.near.org/smart-contracts/anatomy/crosscontract
- Reentrancy implication: "Between a cross-contract call and its callback, any public method of your contract can execute" (paraphrase); mitigation is to debit balances before the call and restore in the callback on failure, and to credit deposits only after success. https://docs.near.org/smart-contracts/security/reentrancy

---

## 5. Substrate pallet-contracts and pallet-revive

- Storage deposit config (pallet-contracts 50.0.0): `DepositPerByte` ("balance a caller has to pay for each byte of storage"), `DepositPerItem` (per storage item), `CodeHashLockupDepositPercent` (share of deposit held for using a code hash), `MaxCodeLen`, `MaxStorageKeyLen`. Changing deposit values on a live chain "might require a storage migration". https://docs.rs/pallet-contracts/latest/pallet_contracts/pallet/trait.Config.html
- Who pays and who is refunded: the storage meter charges "the origin of the call stack ... responsible for putting down a deposit"; balance flows origin to contract on a charge and contract to origin on a refund; refunds are pro rata to the contract's accumulated byte/item deposit. `remove_code` "refunds the deposit to its owner". https://github.com/paritytech/polkadot-sdk/blob/master/substrate/frame/contracts/src/storage/meter.rs , https://docs.rs/pallet-contracts/latest/pallet_contracts/
- Weight: callers set a Weight limit per call; unused weight refunded regardless of outcome; one `ref_time` Weight = one picosecond on the reference machine; running out of weight reverts only the current contract frame, failures do not bubble up. https://github.com/paritytech/polkadot-sdk/blob/master/substrate/frame/contracts/README.md
- Code size: `MaxCodeLen` is runtime-configured; the pallet's default config prelude uses `123 * 1024`, and an integrity check bounds `MaxCodeLen` against runtime heap because wasmi needs a multiple of code size in memory (comment cites a `MaxCodeLen*17*4` margin). https://github.com/paritytech/polkadot-sdk/blob/master/substrate/frame/contracts/src/lib.rs
- Reentrancy: denied by default in ink!/pallet-contracts; enabled per call with `CallFlags::ALLOW_REENTRY` (`set_allow_reentry` on the call builder). Violations yield `ReentranceDenied`. https://github.com/use-ink/ink/issues/1285 , https://www.bittensor.com/docs/errors/chain/ReentranceDenied
- Why Parity moved to pallet-revive: the fork lets them "remove all the code that was necessary to support Wasm" and run cross-compiled YUL (Solidity) with a different ABI on PolkaVM instead of wasmi. https://gitlab.parity.io/parity/mirrors/polkadot-sdk/-/commit/559fa1db0594a81d5dbf343613ba2f3fc16708da
- pallet-revive is "a heavily stripped down and simplified fork of pallet_contracts", replaces chain extensions with precompiles, and uses 20-byte (H160) addresses mapped to AccountId32. https://forum.polkadot.network/t/contracts-on-assethub-roadmap/9513 , https://github.com/paritytech/polkadot-sdk/blob/master/substrate/frame/revive/README.md
- PolkaVM rationale (2023 announcement): interpreter speed (wasmi 108 ms/frame vs PolkaVM 10.7 ms/frame on the NES benchmark), process isolation with seccomp and 9 syscalls as a "second line of defense", small dependency tree, and deterministic gas being hard because cache effects make equivalent code vary 19x. https://forum.polkadot.network/t/announcing-polkavm-a-new-risc-v-based-vm-for-smart-contracts-and-possibly-more/3811
- Downstream signal: Polymesh v8 removed the legacy contracts pallets entirely, noting no contracts were ever deployed on its mainnet through pallet-contracts. https://developers.polymesh.network/development/changelogs/v7.4-to-v8.0/smart-contracts-evm/
- No formal deprecation notice for pallet-contracts was found.

---

## 6. Arbitrum Stylus

- Ink: a sub-gas unit "orders of magnitude smaller"; by default 1 gas buys 10,000 ink, adjustable by the chain owner, so WASM ops can be repriced without touching the EVM schedule. https://docs.arbitrum.io/stylus/concepts/stylus-gas
- Claimed savings vs EVM: compute 10-100x cheaper, memory 100-500x cheaper; memory charged per 64 KB page with an 8 MB (128 page) default limit; calling a Stylus contract costs at least 8,832 gas uncached or 352 gas cached. Same URL.
- Activation: a WASM program must be activated once; cost = configurable `ActivationGas` (default 0) + fixed 1,659,168 gas + a data fee scaling with compiled size; identical later deploys skip activation. Same URL.
- Runtime: Nitro's `arbitrator/stylus` depends on a vendored Wasmer (`tools/wasmer`) with singlepass and cranelift compilers. https://github.com/OffchainLabs/nitro/blob/master/arbitrator/stylus/Cargo.toml
- Interop: Stylus and EVM contracts share one state and call each other; Stylus SLOAD/SSTORE cost the same as EVM; host I/O replaces opcodes for state access. https://docs.arbitrum.io/stylus/concepts/stylus-gas , https://docs.arbitrum.io/how-arbitrum-works/state-transition-function/stylus-execution-path
- Reentrancy: the SDK's `reentrant` feature is opt-in and now marked deprecated: "this guard is redundant (cache flushing provides reentrancy safety) and blocks use cases that require reentrant calls." https://github.com/OffchainLabs/stylus-sdk-rs/blob/main/stylus-sdk/Cargo.toml
- Audits: Trail of Bits Stylus assessment (2024-06-10) plus follow-ups. https://docs.arbitrum.io/assets/files/2024_06_10_trail_of_bits_security_audit_stylus-f2f68cbe59f5ac1c085292f6811c8ac9.pdf , https://trailofbits.com/library/offchain-labs-offchain-stylus/
- What it shows: WASM can sit next to the EVM in one state tree if both share the account/address model and storage cost schedule; the price of that is two VMs, a native compile step (activation) and a Wasmer dependency in consensus.

---

## 7. EVM inside a non-EVM chain

- revm: current stable `revm 43.0.3` (crates.io, 2026-09-24). Used by Reth, Helios, Trin, Foundry, Hardhat, Optimism, Base, Scroll, and zkVMs (Risc0, Succinct); designed both as executor and as a framework for EVM variants. https://crates.io/api/v1/crates/revm , https://github.com/bluealloy/revm
- revm's own users list (awesome-revm) names Ethereum L1/L2 tooling and rollups only; no non-EVM L1 embedding revm next to native modules was found there. https://bluealloy.github.io/revm/awesome.html
- Address mismatch: an Ethereum address is "the last 20 bytes of the Keccak-256 hash of the public key" generated with ECDSA (secp256k1). https://ethereum.org/en/developers/docs/accounts/ . Signature recovery (`ecrecover`, precompile 0x01) only works for secp256k1, so an ed25519 account cannot sign an Ethereum transaction or be recovered from one. https://www.evm.codes/precompiled
- Sei: EVM and Cosmos addresses derive from one secp256k1 key but are only linked after the chain sees the public key (automatic on first signed tx, or `associate()` / `associatePubKey()` via the address precompile at 0x...1004); before that they are separate accounts. Pointer contracts proxy assets across VMs. https://docs.sei.io/learn/accounts , https://docs.sei.io/evm/precompiles/cosmwasm-precompiles/addr.md , https://docs.sei.io/learn/dev-interoperability
- Monad: its own execution client with a custom EVM implementation in C++ (not revm). https://docs.monad.xyz/monad-arch , https://github.com/category-labs/monad
- Berachain: unmodified Ethereum execution clients (Geth, Reth, Erigon, Nethermind) under BeaconKit / CometBFT consensus, so no account mapping problem. https://docs.dune.com/data-catalog/evm/berachain/overview
- Neon EVM on Solana: each Ethereum account maps to a Solana account (PDA) that must be included in the transaction; for Solana users the EVM address is the last 20 bytes of keccak256(Solana pubkey). https://docs.neonevm.org/docs/evm_compatibility/overview , https://neonevm.org/blog/Enabling-Solana-Users-to-Access-EVM-dApps-Running-on-Solana . Neodyme audited Neon. https://www.neodyme.io/reports/Neon-Update-2023.pdf
- pallet-revive's approach (H160 addresses mapped to AccountId32, explicitly rejecting Frontier's and Acala's dual-key schemes) is the closest analogue to an ed25519 chain adding 20-byte addresses. https://forum.polkadot.network/t/contracts-on-assethub-roadmap/9513

---

## 8. SVM / sBPF as an embedded VM

- `solana-sbpf 0.25.0` (crates.io, 2026-09-22), fork of solana-labs/rbpf (itself a fork of qmonnet/rbpf); has an interpreter, an x86_64 JIT, assembler, disassembler and verifier; JIT does not work on Windows. Four SBPF versions (v0-v3) with differing features. https://crates.io/api/v1/crates/solana-sbpf , https://github.com/anza-xyz/sbpf
- `solana-svm 4.3.2` (crates.io, 2026-10-15). Anza's standalone SVM API (2024-06-27): `TransactionBatchProcessor::load_and_execute_sanitized_transactions`; embedders implement `TransactionProcessingCallback` (`get_account_shared_data`, `account_matches_owners`); intended for rollups, off-chain simulation, fraud proofs, Avalanche subnets. https://crates.io/api/v1/crates/solana-svm , https://anza.xyz/blog/anzas-new-svm-api
- Compute units: default 200,000 CU per instruction, max 1,400,000 per transaction; base fee 5,000 lamports per signature. https://solana.com/docs/core/fees
- Account model: transactions declare all state they read or write before execution, which is what lets Sealevel run non-overlapping transactions in parallel. https://solana.com/news/sealevel---parallel-processing-thousands-of-smart-contracts . Transaction size limit 1,232 bytes (v1 format raises it to 4,096). https://solana.com/docs/core/transactions
- JIT/loader risk, incident 1 (Feb 2024): a bug in the LoadedPrograms (JIT program cache) cooperative-loading path in v1.17 caused an infinite loop on replay; with over 95% of stake on 1.17, consensus halted for about five hours. https://solana.com/news/02-06-24-solana-mainnet-beta-outage-report
- Incident 2 (Aug 2024): ELF sanitization omitted a `.text` alignment check; `CALL_REG` into misaligned code could segfault the host, letting an attacker "crash leaders one by one, eventually halting the network". Patched privately to a supermajority before disclosure. https://www.anza.xyz/blog/agave-network-patch-root-cause-analysis , https://www.theblock.co/post/310361/solana-developers-address-critical-vulnerability-with-coordinated-patch
- SVM outside Solana: Eclipse (SVM execution, Ethereum settlement, Celestia data, RISC Zero proofs; public mainnet Nov 2024). https://blockworks.co/news/eclipse-svm-ethereum-mainnet . Sonic (HyperGrid SVM grids settling to Solana). https://hyperlane.xyz/post/the-state-of-svm-expansion
- Fit for Cadre: sBPF's up-front account list matches a Solana light-client mental model, but the 2023 benchmark had the rbpf interpreter 64x slower than wasmi (6930 vs 108 ms/frame) and both Solana incidents above were in the loader/JIT layer. https://forum.polkadot.network/t/announcing-polkavm-a-new-risc-v-based-vm-for-smart-contracts-and-possibly-more/3811

---

## 9. Reentrancy history and WASM-chain stances

- The DAO (June 17, 2016): a reentrancy bug let an attacker drain about 3.6 million ETH (about a third of the fund) into a child DAO; Ethereum hard forked to return funds, and the original chain continued as Ethereum Classic. https://www.theblock.co/post/405248/ethereum-130-million-security-fund-the-dao-hack-10-years
- Curve / Vyper (July 30, 2023): Vyper 0.2.15, 0.2.16 and 0.3.0 allocated a separate storage slot per `@nonreentrant` decorator instead of one per key, so the lock did not protect across functions; fixed in 0.3.1 without notifying protocols. CVE-2023-39363 (critical). https://hackmd.io/@vyperlang/HJUgNMhs2 , https://www.wiz.io/vulnerability-database/cve/cve-2023-39363
- Curve losses: CertiK reports $69.3M affected, $16.7M returned by white hats, about $52M net stolen. https://www.certik.com/resources/blog/2qbPMcyJpR5UfoKrcjWxlQ-vyper-incident-anaylsis
- Lesson for Cadre: a compiler-level guard failed silently; the guard belongs in the host/VM, not the language.
- CosmWasm: no synchronous cross-contract calls at all (actor model, messages run after return). Section 3 URLs.
- ink! / pallet-contracts: synchronous calls, reentrancy denied by default, opt in with `ALLOW_REENTRY`. Section 5 URLs.
- NEAR: async promises, so classic reentrancy is replaced by interleaving between call and callback. Section 4 URLs.
- Stylus: SDK guard optional and deprecated; relies on storage cache flushing. Section 6 URL.

---

## 10. Faster blocks for a PoW chain

**Shorter PoW target (cost: orphans)**
- Decker and Wattenhofer (IEEE P2P 2013): median block reception 6.5 s, mean 12.6 s, 5% of nodes still without the block after 40 s; observed Bitcoin fork rate 1.69% at 600 s blocks; their relay change lowered it to 0.78%. https://tik-old.ee.ethz.ch/file/49318d3f56c1d525aabf7fda78b23fc0/P2P2013_041.pdf (mirror: https://www.dpss.inesc-id.pt/~ler/docencia/rcs1314/papers/P2P2013_041.pdf)
- First-order rule from the bitcoin-dev list: orphan rate is about propagation time / block interval. https://lists.linuxfoundation.org/pipermail/bitcoin-dev/2013-November/003742.html . Using the 6.5 s median: at 60 s blocks this gives roughly 11% (6.5 / 60). This is Cadre's own arithmetic from the cited figures, not a measurement, and the linear formula stops holding once the interval gets close to the propagation time.
- Ethereum PoW: 7.41% uncle rate over the first 280,000 blocks (2015). https://blog.ethereum.org/2015/09/25/more-uncle-statistics . A later paper reports a non-height-increasing block rate between 0.06 and 0.24 during 2017, at about 15 s average block time. https://arxiv.org/pdf/1805.08832
- GHOST (Sompolinsky, Zohar): at high block rates "substantially weaker attackers" can reverse payments under longest-chain; counting off-chain subtree blocks restores security; claims rates above one block per second. https://eprint.iacr.org/2013/881

**Pre-confirmations / weak blocks**
- Subchains / weak blocks (Rizun, Ledger 2016): fractional-difficulty blocks between real blocks to cut orphan risk and give faster weak confirmations, no consensus change. https://ledger.pitt.edu/ojs/ledger/article/view/40 . Critique: no subchain-level security analysis, open to selfish mining on the subchain. https://arxiv.org/pdf/1905.09655
- Ergo sub-blocks (2023 proposal): at 1/128 of block difficulty, about one sub-block per second; base target block interval 120 s; not confirmed deployed. https://ergoplatform.org/en/blog/Ergo%E2%80%99s-Newest-Advancement-Sub-Blocks/ , https://docs.ergoplatform.com/mining/difficulty/
- Bobtail (Bissias, Levine, NDSS 2020): block needs the k lowest hashes instead of one; inter-block variance falls about as 1/k; example: 40% attacker double-spend success 53% at k=1 vs under 1% at k>=40; costs are larger blocks and more traffic. https://arxiv.org/pdf/1709.08750 , https://www.ndss-symposium.org/wp-content/uploads/2020/02/23095.pdf
- Ethereum based preconfirmations (Drake, 2023-11-08): proposer-issued promises "on the order of 100ms", requiring opt-in proposer slashing (restaking) and forced inclusion. https://ethresear.ch/t/based-preconfirmations/17353 . For PoW there is no bonded proposer, so a preconfer would need a separate stake.

**Leader-based microblocks**
- Bitcoin-NG (NSDI 2016): PoW key blocks elect a leader (10 min exponential) who signs microblocks on a short deterministic interval (10 s in the talk); latency bounded by network propagation. https://www.usenix.org/node/194907 , https://usenix.org/sites/default/files/conference/protected-files/nsdi16_slides_eyal.pdf

**Finality gadgets and committees over PoW**
- Hybrid Casper FFG (EIP-1011): PoS finality contract over PoW, checkpoint every 50 blocks; status stagnant/abandoned in favor of full PoS. https://eips.ethereum.org/EIPS/eip-1011
- Hybrid Consensus (Pass, Shi, DISC 2017): slow PoW chain elects a committee that runs fast BFT; responsive after warmup if under 1/3 of committee is corrupt and delay bound is known. https://eprint.iacr.org/2016/917
- Babylon: timestamps PoS checkpoints on Bitcoin and lets BTC stake (slashable via EOTS) secure PoS chains; gives Bitcoin-backed finality to PoS, not to a PoW chain directly; paper claims unbonding cut from weeks to under 5 hours. https://docs.babylonlabs.io/guides/overview/bitcoin_staking/research/btc_timestamping/ , https://arxiv.org/abs/2201.07946

**BlockDAG**
- Kaspa Crescendo hard fork (May 5, 2025) raised GHOSTDAG from 1 to 10 blocks per second with per-block reward cut 10x. https://medium.com/@coderofstuff/post-crescendo-mining-observations-50b99f36721a
- Kaspa states transactions confirm after about 10 s under normal conditions on GHOSTDAG; DAGKNIGHT (eprint 2022/1494) is parameterless and adapts to network latency, tolerating up to 50% hashrate corruption; proof details still being completed per its author. https://eprint.iacr.org/2022/1494 , https://kaspa.org/?p=44260
- Tradeoff: DAG ordering changes the state machine (conflicting transactions in parallel blocks), so contract execution order must be derived from the DAG linearization; that is a deep change compared with weak blocks or a finality gadget.

---

## 11. AI agents writing contracts

- Do, Sohrabi, Hassan, "Evaluating the Vulnerability Landscape of LLM-Generated Smart Contracts" (arXiv 2602.04039, Feb 2026): Solidity contracts from ChatGPT, Gemini and Sonnet often compile and work but "frequently exhibit severe security flaws". No numeric rates in the abstract. https://arxiv.org/abs/2602.04039
- SmartEval (arXiv 2605.09610, 2026): benchmark for generating contracts from natural-language specs, checked with Slither. https://arxiv.org/pdf/2605.09610
- arXiv 2511.16224: benchmarks plausibility, gas and complexity of LLM-generated contracts (mostly Solidity); retrieval-augmented prompting helps. https://arxiv.org/pdf/2511.16224
- Rust side: arXiv 2511.11250 tests LLM vulnerability detection on Solana Rust and Algorand PyTeal snippets (detection, not generation). https://arxiv.org/html/2511.11250v1 . arXiv 2501.12972 uses LLMs to turn CosmWasm contracts into Quint models for verification. https://arxiv.org/pdf/2501.12972
- Not found: any head-to-head study of bug rates in LLM-written Solidity vs LLM-written Rust contracts, and anything on ink! or wasmi-hosted contracts.

---

## 12. Audit firm candidates (WASM VM hosts)

- SRLabs: wasmi 0.31.0 for Parity. https://github.com/wasmi-labs/wasmi/blob/main/resources/audit-2023-12-20.pdf . Also ink! 4.0 and the contracts pallet (2022-2023). https://polkadot.subsquare.io/treasury/proposals/908
- Runtime Verification: wasmi 0.36-0.38 for Stellar. https://github.com/wasmi-labs/wasmi/blob/main/resources/audit-2024-11-27.pdf
- Oak Security: CosmWasm, wasmvm (2023-03-27), wasmd (2023-10-17), Sei chain + CosmWasm (2023-05-19). https://github.com/oak-security/audit-reports/tree/main/CosmWasm
- Trail of Bits: Arbitrum Stylus WASM runtime (2024) and Nitro arbitrator/WAVM (2022). https://docs.arbitrum.io/assets/files/2024_06_10_trail_of_bits_security_audit_stylus-f2f68cbe59f5ac1c085292f6811c8ac9.pdf , https://docs.arbitrum.io/assets/files/2022_03_14_trail_of_bits_security_audit_nitro_1_of_2-d777111730bd602222978f7d98713d40.pdf
- CertiK: published the CWA-2023-004 compile-bomb write-up. https://www.certik.com/resources/blog/risk-and-security-enhancement-for-app-chains-an-in-depth-writeup-of-cwa-2023
- Neodyme: Neon EVM (EVM-on-SVM). https://www.neodyme.io/reports/Neon-Update-2023.pdf
- Halborn: "Wasm integration" assessment for Umee. https://halborn.com/audits/umee/wasm-integration-cosmos-security-assessment
- Not found: a third-party audit of wasmtime itself (Trail of Bits or otherwise).

---

## Implications for Cadre

1. wasmi is the defensible default: interpreter (no native codegen, so no Cranelift-class miscompile/sandbox escapes like CVE-2023-26489 or CVE-2026-34971), already used on chain by Soroban, two public audits. But both wasmi advisories were memory-safety bugs (UAF in memory growth, OOB write over 128 params), so pin `wasmi = "=2.0.0"`, track GHSA, and cap memory.
2. Configure for determinism explicitly, since defaults are permissive: `consume_fuel(true)`, `floats(false)`, build without the `simd` feature (relaxed-simd is on by default otherwise), disable memory64, multi-memory, reference types, tail calls unless needed; `CompilationMode::Eager` or `LazyTranslation`, never `Lazy`; `EnforcedLimits::strict()`; `StoreLimits` with `memory_size` set and `trap_on_grow_failure(true)`; explicit `set_max_recursion_depth` and `set_max_stack_height`.
3. Replace the flat default fuel table (1 per op) with a benchmarked `OperatorCost` table and `CustomFuelCosts`, and charge deploy-time fuel per byte validated/translated. Version the table in consensus params; CosmWasm (CWA-2024-007) and wasmtime (GHSA-m63x-6p34-q65x) both shipped metering bugs that became DoS vectors.
4. Compile bombs are real even for interpreters' translators: CWA-2023-004 hit GB-scale artifacts, NEAR saw 500 MB and 7.6 s Cranelift compiles. Use deterministic structural limits (function count, params, body size, blocks), never wall-clock timeouts, and charge upload by size.
5. Reentrancy: deny by default in the host (ink! style call stack check), with an opt-in flag; or adopt CosmWasm's actor model (no synchronous calls). Do not leave it to an SDK macro (Vyper 2023).
6. State rent: adopt a per-byte plus per-item deposit charged to the call-stack origin and refunded pro rata on deletion (pallet-contracts model) or NEAR-style staking (1E19 yN/byte as a reference point). Set code size cap by measurement; references: CosmWasm 800 KiB, NEAR 4 MiB, Stylus page-based 8 MB memory.
7. EVM is not a cheap add-on: ed25519 accounts cannot `ecrecover`, so EVM would need a second secp256k1 account type plus a mapping (Sei association, Neon PDA, pallet-revive H160 mapping). Defer; if wanted later, revm 43.x is the embeddable option.
8. sBPF/SVM fits a Solana-shaped account list but brings JIT/loader risk (two network-halting Solana bugs in 2024) and its interpreter was the slowest option benchmarked. Not recommended as the primary VM.
9. Faster UX without shrinking PoW target: at the cited 6.5 s median propagation, 60 s blocks are already near 11% orphan risk by the first-order formula; going lower costs security (GHOST). Prefer weak blocks / sub-blocks (Rizun, Ergo) or Bobtail-style k-hash blocks for soft confirmations, or a stake-based finality gadget (Casper FFG over PoW, Hybrid Consensus); BlockDAG (Kaspa) is a bigger redesign.
10. LLM-authored contracts: evidence shows LLM Solidity often compiles but carries severe flaws; no comparative Rust data exists. Plan host-level guards, a mandatory test/fuzz harness in the SDK, and an audit by one of SRLabs, Runtime Verification, Oak Security or Trail of Bits before mainnet.
