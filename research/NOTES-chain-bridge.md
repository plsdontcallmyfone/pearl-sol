# Agent chain + bridge research notes

Research subagent, 2026-10-16. Scope: how to run the agent chain (own SVM cluster),
how the bridged pump.fun token becomes its fee token, how to add a native verifier
builtin later, which bridge to use, and what agent-economy primitives already exist.
Local toolchain: solana-cli 3.1.12 (Agave, feat:4140108451), anchor 0.31.1.
Confidence tags: [verified] = read in source or official docs this session;
[reported] = secondary source; [inference] = my reasoning, not stated anywhere.

## TL;DR recommendations

1. **Chain: plain Agave cluster from our own genesis, forked Agave repo from day one**
   (even if the fork carries zero diffs at first). Not a rollup framework. None of
   MagicBlock, Sonic HyperGrid, Termina, SOON or Eclipse give "own chain, own fee
   token, own native builtin" more cheaply than a forked Agave does, and most of them
   are either Solana-sidecar sessions (MagicBlock) or stalled/pivoted (Sonic, Termina).
2. **Fee token: the bridged token IS native lamports.** Do not try to make an SPL mint
   the gas token. Pre-allocate the entire 1B supply as lamports at genesis into the
   bridge's escrow account on the agent chain, set `--inflation none` and
   `--fee-burn-percentage 0`, and have the bridge *unlock* lamports on deposit and
   *re-lock* them on withdrawal. Supply on the agent chain can never exceed 1B, and
   there is no mint authority to steal. 1 token (6 dp on pump.fun) = 1e9 lamports, so
   1 SPL base unit = 1000 lamports; 1B tokens = 1e18 lamports, fits u64 (max 1.8e19).
3. **Native builtin: easy to add in a fork, no feature gate needed on our own chain**
   (register it with `enable_feature_id: None` so it is live from genesis), but it
   becomes consensus code. Pearl's verifier is a plonky2/starky STARK verify
   (`research/pearlrepo/zk-pow/src/api/verify.rs::verify_block`), which will not fit
   in a BPF program (1.4M CU tx cap, 32 KiB default heap, 256 KiB max). So: M1 verifies
   off-chain (M-of-N attesters sign "proof ok for job X", on-chain program checks sigs
   with the ed25519 precompile); M2/M3 moves `verify_block` into a native builtin or a
   custom syscall.
4. **Bridge: M1 = our own lock/unlock programs + a relayer with M-of-N ed25519 signers.**
   **Production = Hyperlane Sealevel, self-hosted** (our own Multisig ISM validator
   set + our own relayer), Solana side `collateral` (locks the pump.fun SPL), agent
   chain side `native` (releases pre-funded lamports). Wormhole and LayerZero are not
   self-serve for a brand-new chain.
5. **Agent economy: copy designs, do not import code.** ERC-8183 (job escrow with an
   evaluator that may be a contract verifying a proof) maps one-to-one onto "paid job +
   useful-PoW proof". Solana Agent Registry (8004 on Solana) for identity. x402 "exact"
   SVM scheme works unchanged on our chain for SPL transfers; for native-lamport
   payments we need a small variant.

---

## 1. Running an independent SVM cluster

### 1a. Agave multi-validator with custom genesis [verified]

Binaries. Anza stopped publishing `agave-validator` prebuilt binaries as of Agave
v3.0.0; operators build from source (`./scripts/cargo-install-all.sh .`). Our local
install has `solana`, `solana-test-validator`, `agave-ledger-tool` but **no
`solana-genesis` and no `agave-validator`**; both come from building the repo. Build
target is Ubuntu 24.04 x86_64. Since we fork anyway (section 2), building from source
costs nothing extra. Pin the fork to the tag matching our CLI (v3.1.12) for M1 so
client and validator agree, then track upstream stable (Agave 4.2 is live on mainnet).

`solana-genesis` flags (checked in v3.1.12 source, `genesis/src/main.rs`):

| Flag | Use for us |
|------|------------|
| `--cluster-type development` | activates **all** features, no RPC check. Use this. (devnet/testnet/mainnet-beta copy that cluster's live feature set via `--url`) |
| `--deactivate-feature <pubkey>` (repeatable) | turn off anything we do not want; works with any cluster type |
| `--bootstrap-validator <identity> <vote> <stake>` (repeatable) | genesis validators |
| `--validator-accounts-file <yaml>` | extra genesis validators with balances |
| `--bootstrap-validator-lamports`, `--bootstrap-validator-stake-lamports` | keep small (see supply accounting below) |
| `--bootstrap-validator-bls-pubkey`, `--alpenglow` | present in v3.1.12 for Alpenglow consensus; leave off (see below) |
| `--primordial-accounts-file <yaml>` | **pre-fund the bridge escrow with 1e18 lamports**, plus any program-owned state |
| `--bpf-program ADDR LOADER SO` / `--upgradeable-program ADDR LOADER SO AUTH` | install the bridge, job-market and mining programs at fixed addresses from genesis |
| `--inflation none` | no staking inflation, so supply is fixed |
| `--fee-burn-percentage 0` | default burns 50% of fees; burning would shrink the bridged supply below what is locked on Solana |
| `--target-lamports-per-signature`, `--target-signatures-per-slot` | fee level; 0 sigs-per-slot disables fee adjustment |
| `--lamports-per-byte` | rent-exempt cost; tune so account creation is cheap in token terms |
| `--slots-per-epoch`, `--enable-warmup-epochs`, `--hashes-per-tick`, `--ticks-per-slot` | timing; `--hashes-per-tick auto` is fine |
| `--faucet-pubkey` + `--faucet-lamports` | **do not use in production** (would mint unbacked supply); fine for localnet |
| `--vote-commission-percentage` | irrelevant if we run all validators |

Staking and supply accounting [inference]. Stake is denominated in native lamports, so
every lamport staked at genesis is part of the 1B. Rule: total lamports at genesis =
1e18 exactly = escrow + validator identity/vote/stake balances, and the team locks the
matching SPL amount on Solana for whatever is outside the escrow. A permissioned
cluster does not need meaningful stake (stake only weights leader schedule and votes),
so give each validator e.g. 1 to 10 tokens of stake. Vote transactions pay fees
(5000 lamports/sig at default, about 2 votes/s/validator, roughly 0.86 token/day per
validator at 1e9 lamports per token); with fee burn at 0 that just recirculates to
leaders. Under Alpenglow votes stop being transactions, but Alpenglow is not yet on
mainnet (Agave 4.2 did not ship it; 4.3 testing; a reported Sept 28 activation was
denied) so stay on TowerBFT.

Ops shape [verified for mainnet specs, inference for ours]. Anza's mainnet validator
spec is 12c/24t at 2.8 GHz+ with SHA ext and AVX2, 512 GB RAM capable board, separate
NVMe for accounts (1 TB) and ledger (1 TB), 2 to 10 Gbit/s. That is sized for mainnet
state and load. A private cluster with small state and modest TPS runs on much less:
plan on 8 to 16 cores, 64 to 128 GB RAM, 2 NVMe, 1 Gbit/s per node, which is roughly
USD 150 to 400/month per dedicated box (Hetzner/OVH/Latitude class). Production
minimum: 3 to 4 voting validators in different providers (TowerBFT needs >2/3 stake
for finality, so 4 nodes tolerate 1 down) plus 1 to 2 non-voting RPC nodes with
`--full-rpc-api` and limited ledger (`--limit-ledger-size`). Expect a realistic
USD 1k to 2k/month for 4 validators + 2 RPC, plus monitoring
(solana-watchtower or our own), snapshot hosting for new nodes, and a runbook for
cluster restarts (`--wait-for-supermajority` + `--expected-bank-hash` is the
documented restart path after a halt). Upgrades are coordinated by us, which is the
point: features and builtins are activated whenever we ship a new binary.

Precedent: Fogo (Firedancer-based SVM L1, mainnet 2026-01-15) runs its own genesis,
own native gas token FOGO, a curated validator set, and Wormhole as its bridge. That
is the same architecture as ours at larger scale. Solana Foundation's "Solana
Permissioned Environments" (SPE) is the same idea marketed to enterprises: a private
Agave cluster where the operator is the highest authority.

### 1b. Alternatives

| Option | What it really is (Oct 2026) | Fit for "own chain, own fee token, native builtin later" |
|--------|------------------------------|-----------------------------|
| **Forked Agave cluster** | full L1, our genesis, our validators | **Best.** All three requirements are first-class. Cost: ops. |
| MagicBlock Ephemeral Rollups | accounts delegated from Solana to an ER validator, state committed back; fees in SOL (session fee 0.003 SOL, commit fees; Validator v1.0 released 2026-10-06) | Poor. It is a sidecar session on Solana, not a sovereign chain; fee token is SOL; you do not add builtins. Good for low-latency game loops, not this. |
| Sonic SVM / HyperGrid | Mobius mainnet Feb 2025 used SOL for gas; project has pivoted to "Attention Capital Markets" and agent sessions in 2026; no evidence of third-party HyperGrid chains | Poor. Unclear framework status, SOL-gas, vendor dependency. |
| Eclipse | SVM L2 on Ethereum with Celestia DA; ETH is native gas (ETH shows up as lamports at the native mint `So111...111`), reportedly moving to ES via a paymaster | Proves the pattern "bridged asset = native lamports", but its stack is Ethereum-settled and not offered as a framework. |
| SOON | SVM L2 framework settling to Ethereum, "decoupled SVM" | Wrong settlement layer for a Solana-native token. |
| Termina (Nitro Labs) | "network extensions" toolkit (SVM Engine, zkSVM, Data Anchor); last concrete news is the Oct 2024 seed, status "announced" | Too immature to depend on. |
| Anza `solana-svm` crate (SVM API) | `TransactionBatchProcessor::load_and_execute_sanitized_transactions` + a `TransactionProcessingCallback` you implement for account storage; no consensus, no networking, no accounts DB, no fee policy | Viable only if we want a single-sequencer appchain and are willing to write sequencing, storage, RPC, and block production ourselves. Interesting for a later "agent chain as a rollup" phase; far more work than running Agave for M1. Builtins can be registered via its builtin program id set / `add_builtin_account` hook. |

Verdict: run Agave. The SVM API is the fallback if validator ops become the bottleneck
and we accept a single sequencer.

### 1c. Can the fee token be the bridged SPL token instead of native lamports?

Not natively. Agave charges the fee payer's **lamports** in the bank's fee-collection
path; there is no configuration to charge an SPL balance. Workarounds and why we skip
them:

- Paymaster / relayer (Kora, x402 facilitators): a fee payer pays lamports and the user
  reimburses in SPL. Works on Solana, but on our chain it just means "someone must
  still hold the native token", so it does not remove native lamports.
- Fork the fee logic to debit an SPL account: deep, consensus-critical change in the
  bank's fee path and every client/wallet's fee estimation. Not worth it.

**Practical pattern (recommended): native lamports ARE the bridged token, backed 1:1
by locked SPL on Solana.**

- Genesis: a single account (the bridge escrow PDA on the agent chain, owned by our
  bridge program, or the Hyperlane `token-native` escrow in production) holds
  1e18 lamports minus genesis stake. Use `--primordial-accounts-file` to place it.
- Deposit (Solana to agent chain): user locks N SPL base units in the Solana bridge
  vault; relayer proves it; agent-chain program moves N * 1000 lamports from escrow to
  the user. **Unlock, not mint**: no mint authority exists anywhere.
- Withdraw: user sends lamports into the escrow via the bridge program; relayer proves
  it; Solana program releases N SPL.
- Invariant to monitor continuously: `SPL locked in Solana vault * 1000 ==
  agent-chain total lamport supply - escrow balance`. Both sides are observable; alert
  on any drift. Holds as long as inflation is none and fee burn is 0. (Rent is now
  effectively rent-exempt-only on current Agave, so no rent collection leaks supply.)
- The mining reserve is just SPL held in the reserve wallet on Solana, bridged once
  into a mining-reserve PDA on the agent chain as lamports.
- Wallets (Phantom, Backpack, CLI) show native lamports as "SOL" with 9 decimals on a
  custom RPC. Cosmetic issue only; our own UI labels it.

---

## 2. Adding a native builtin to a forked Agave

Difficulty: moderate. The code changes are small; the risk is that it is consensus code
that every validator must run identically, and that a panic or nondeterminism halts
the chain.

Where (Agave master/v3.x layout) [verified for the files read; exact helper names may
drift between versions]:

- `builtins/src/lib.rs`: the `BUILTINS: &[BuiltinPrototype]` table. Each entry has
  `name`, `program_id`, `enable_feature_id: Option<Pubkey>`, `core_bpf_migration_config`,
  and `register_fn` (the entrypoint). Current entries: system, vote, the three BPF
  loaders, compute budget, zk_token_proof (gated), zk_elgamal_proof (gated).
  `STATELESS_BUILTINS` is empty. **Add our `pearl_verify` entry here.**
- `programs/<our-builtin>/`: new crate. The entrypoint uses the program-runtime
  `declare_process_instruction!(Entrypoint, DEFAULT_COMPUTE_UNITS, |invoke_context| ...)`
  pattern the existing builtins use (see `programs/zk-elgamal-proof` as the closest
  template: a stateless verifier builtin that consumes CUs per proof type).
- `builtins-default-costs/src/lib.rs`: `NON_MIGRATING_BUILTINS_COSTS` /
  `MIGRATING_BUILTINS_COSTS` feed `BUILTIN_INSTRUCTION_COSTS` and the
  `MAYBE_BUILTIN_KEY` byte filter used by the cost model and compute-budget defaults.
  Upstream comments say "do not add entries" because it changes consensus on mainnet;
  on our own chain we add `(pearl_verify::id(), BuiltinCost::NotMigrating)` and bump the
  `TOTAL_COUNT_BUILTINS` test constant. If omitted, the builtin is treated like a
  non-builtin for default-CU accounting (200k default per instruction), which works but
  makes block cost estimation less accurate [inference].
- `sdk-ids` style id constant for the program address (we can keep it in our own crate).

Feature gate: **not required on our chain.** `enable_feature_id: None` means the
builtin is registered from slot 0 in every bank. Use a feature gate only when adding the
builtin to an already-running cluster, so that all validators switch at the same epoch:
create a feature keypair, set `enable_feature_id: Some(feature_id)`, ship the binary to
all validators, then `solana feature activate` it. With `--cluster-type development`
genesis, a feature that is known to the binary at genesis time is activated
automatically, so a fresh chain can get it from block 0 either way.

Alternative with a smaller blast radius: **a custom syscall** instead of a builtin
program. Keep the job/reward logic in an Anchor program and expose one syscall, e.g.
`sol_pearl_verify(proof_ptr, len, public_params_ptr) -> u64`, registered where the
runtime builds the BPF program environment (`syscalls` crate,
`create_program_runtime_environment_v1`) and charged a fixed CU cost. Anchor programs
then call it via an `extern "C"` stub. Same consensus implications, but the reward and
escrow logic stays upgradeable BPF and only the hot math is native. This is my preferred
M3 shape [inference].

Why we need it at all: Pearl verification is `zk-pow::api::verify::verify_block`, a
plonky2/starky proof check with a circuit cache. A STARK verify in sBPF would blow past
the 1.4M CU transaction limit and the heap limit by a wide margin (Solana's own ZK
verification for Token-2022 confidential transfers lives in a native builtin for the
same reason). Determinism checklist before shipping: no floats, no rayon/threads, no
wall-clock, bounded input size checked before any allocation, never panic (map every
error to `InstructionError`), pin plonky2 versions and the circuit cache bytes.

---

## 3. Bridge options (Solana <-> our SVM chain)

### Hyperlane Sealevel [verified, docs + monorepo]

- Programs in `hyperlane-monorepo/rust/sealevel/programs`: `mailbox`,
  `validator-announce`, `ism` (multisig message-id, composite), `hyperlane-sealevel-igp`,
  `hyperlane-sealevel-token` (synthetic), `-token-collateral`, `-token-native`,
  `-token-cross-collateral`, `hyperlane-sealevel-fee`.
- `hyperlane-sealevel-client` has `core deploy` (takes `--chain`, `--local-domain`,
  `--remote-domains`, global `--url`), `multisig-ism-message-id` (deploy, set
  validators/threshold), `validator-announce`, `igp`, `warp-route deploy`.
- Warp routes are permissionless; you choose your own ISM validator set. Hyperlane's
  public SVM guide says to "get in touch" to have a new SVM chain become an *official*
  core deployment (listed in the registry, served by Abacus Works' validators/relayer).
  We do not need that: self-host core on the agent chain, run our own Hyperlane
  validators (signers) and one relayer (agents support `sealevel` protocol chains via
  custom chain config) [reported + inference; confirm with a localnet run].
- Route shape for us: **Solana `collateral` (pump.fun SPL, classic Token program) <->
  agent chain `native` (lamports)**. The native program holds lamports in an escrow and
  releases them, which is exactly the "unlock pre-funded lamports" pattern from 1c;
  pre-fund its escrow PDA at genesis. This combo is a standard route type
  (native/collateral/synthetic endpoints compose freely), but I did not find a
  documented collateral-to-native example; verify decimals handling (6 vs 9) in
  `token-native` config (Hyperlane routes carry a per-side `decimals` and scale).
- Gotchas: the SVM tooling pins **solana-cli 3.0.14** ("must use this version") and
  its build script switches toolchains; we are on 3.1.12, so run it in a separate
  install dir and never touch the global `solana config`. Deployer key becomes program
  upgrade authority; move it to a multisig (Squads exists only on Solana side; on our
  chain use our own multisig or burn authority after audit). Budget ~5 SOL-equivalent
  rent per side for deployment. Solana side also needs the Solana core, which already
  exists (Solana mainnet is an official Hyperlane chain), so only our chain needs a
  core deploy.

### Wormhole

New chains need a Wormhole Core contract on the chain **and every Guardian (19) must
run a full node of it**; adding a chain is a Guardian governance action (2/3+). Fogo got
this as a funded L1 partner. NTT custom transceivers are additive to Guardian
attestation, not a replacement. Not available to a small private chain on our
timeline.

### LayerZero

Solana Endpoint/ULN/OFT are LayerZero-Labs-deployed programs; new chains are added by
LayerZero Labs, not self-serve. (The "Sonic" in LZ configs is the EVM Sonic, ex-Fantom,
not Sonic SVM.) Not viable for a private SVM chain.

### Own lock/unlock programs + relayer with M-of-N signers

- Solana side: Anchor program with an SPL vault (PDA ATA), `deposit(amount,
  recipient_on_agent_chain)` emitting an event with a monotonically increasing nonce,
  and `release(msg, sigs)` that checks M-of-N ed25519 signatures via the ed25519
  precompile (instruction introspection), with a replay bitmap / nonce PDA.
- Agent chain side: mirror program owning the lamport escrow.
- Relayer: watches finalized slots on each side, signs `(src_chain, nonce, recipient,
  amount)`; M1 can be 1-of-1, production 3-of-5 on separate machines/HSMs.
- Pros: tiny, fully understood, works with any solana-cli version, no third-party
  protocol risk, matches what we already built for memechain (relayer + Anchor bridge
  program, e2e verified). Cons: we own the security and the audit; a bespoke M-of-N
  bridge is a classic hack target.

### Recommendation

- **M1 localnet: own lock/unlock programs + 1-of-1 (then 2-of-3) relayer.** Two
  `solana-test-validator` instances on our port block (10600 and 10630), no toolchain
  pin conflicts, fastest iteration, and the same message format can be kept behind an
  interface so the transport can be swapped later.
- **Production: Hyperlane Sealevel, self-hosted** (own Multisig ISM signers 3-of-5 or
  more, own relayer, Solana `collateral` <-> agent chain `native`). Reasons: audited
  programs already live on Solana mainnet, permissionless, a standard warp-route UX and
  explorers, and it gives us EVM/Cosmos reach for free later. Before committing, run
  Hyperlane's `rust/main/utils/run-locally` sealevel e2e (it has a `sealevel` module
  that drives local Solana validators) against our genesis to confirm the native-escrow
  pre-funding and 6-to-9 decimal scaling.

---

## 4. Agent economy primitives (brief)

- **Job escrow: ERC-8183 "Agentic Commerce"** (draft, 2026-02-25). Job states
  Open -> Funded -> Submitted -> Terminal; only the *evaluator* can complete and release
  escrow; evaluator may be a contract "verifying a zero-knowledge proof". Optional
  attestation hash on complete/reject composes with ERC-8004 reputation; refund cannot
  be blocked by hooks. Reference impl `qntx/market-contract` (EVM) and a Rust SDK
  `erc8183`. **This is our job market**: client = agent posting the AI job, provider =
  miner/worker, evaluator = the Pearl-proof verifier (attester set in M1, builtin later).
  Port the state machine to Anchor; no Solana implementation found.
- **Identity: Solana Agent Registry** (Solana Foundation, 8004 on Solana): PDA-backed
  agent identity + registration file, reputation and validation registries,
  interoperable with ERC-8004; tooling 8004 Scan, 8004 Market, SATI dashboard. Deploy
  the same program on our chain (it is just a Solana program) so agent ids are portable.
- **Payments: x402 on Solana.** "exact" SVM scheme: server returns 402 with mint,
  payee, amount, fee payer; client partially signs a `TransferChecked` (SPL / Token-2022)
  + memo; facilitator co-signs as fee payer (Solana Foundation recommends Kora for
  policy) and submits. Crates: `x402-chain-solana`, `r402-svm`, `r402-solana`.
  On our chain this works as-is for any SPL token, but our value token is **native
  lamports**, so either (a) add a "native exact" variant (system transfer + memo), or
  (b) wrap to wSOL-style native mint (`So111...112`) and keep the SPL flow. (b) needs
  zero protocol changes; recommend (b) for M1.
- Compute/job-market precedent on Solana: Nosana (GPU job market on Solana) is the
  closest live example; I did not confirm its program source in this pass.

---

## Sources

Agave / genesis / builtins
- Agave `genesis/src/main.rs` (v3.1.12 and master): https://raw.githubusercontent.com/anza-xyz/agave/v3.1.12/genesis/src/main.rs
- Agave `builtins/src/lib.rs`: https://raw.githubusercontent.com/anza-xyz/agave/master/builtins/src/lib.rs
- Agave `builtins-default-costs/src/lib.rs`: https://raw.githubusercontent.com/anza-xyz/agave/master/builtins-default-costs/src/lib.rs
- solana-builtins crate docs: https://docs.rs/solana-builtins
- Anza validator requirements: https://docs.anza.xyz/operations/requirements
- Agave v3.0 stops publishing validator binaries (v2.3.9 notes): https://newreleases.io/project/github/anza-xyz/agave/release/v2.3.9
- Anza install docs mirror: https://www.mintlify.com/anza-xyz/agave/installation
- Alpenglow status: https://solana.com/upgrades/agave-4-2-release-overview , https://www.helius.dev/blog/agave-v4-0 , https://www.kucoin.com/news/flash/solana-developers-deny-alpenglow-mainnet-activation-on-september-28

SVM frameworks
- Anza SVM API: https://anza.xyz/blog/anzas-new-svm-api
- SOON / decoupled SVM: https://blockeden.xyz/blog/2026/03/11/soon-network-svm-liberation-solana-execution-decoupled/
- MagicBlock fees: https://docs.magicblock.gg/pages/ephemeral-rollups-ers/introduction/fees-and-commit-economics
- MagicBlock Validator v1.0: https://solanacompass.com/news/magicblock-releases-validator-v10-for-ephemeral-rollups-on-solana-with-new-session-and-commit-fees
- Sonic Mobius mainnet (SOL gas): https://www.dlnews.com/external/sonic-launches-mobius-mainnet-as-solanas-first-svm-chain-extension
- Sonic 2026 pivot: https://solanacompass.com/projects/sonic
- Termina: https://decrypt.co/289221/nitro-labs-secures-4m-for-termina-pioneering-solana-vm-network-extensions , https://thegrid.id/profiles/Termina
- Eclipse ETH gas at native mint: https://docs.orca.so/orca-eclipse-documentation/eclipse-launch-faq
- SVM landscape: https://www.hyperlane.xyz/post/svm-expansion-the-landscape-beyond-solana
- Solana Permissioned Environments: https://solana.com/developers/guides/permissioned-environments , https://www.helius.dev/blog/solana-permissioned-blockchains
- Fogo + Wormhole: https://wormhole.com/blog/fogo-mainnet-launches-with-interoperability-powered-by-wormhole , https://www.coingecko.com/learn/what-is-fogo

Bridges
- Hyperlane SVM warp route guide: https://docs.hyperlane.xyz/docs/guides/warp-routes/svm/svm-warp-route-guide
- Why build your SVM chain with Hyperlane: https://www.hyperlane.xyz/post/why-build-your-svm-chain-with-hyperlane
- Sealevel programs: https://github.com/hyperlane-xyz/hyperlane-monorepo/tree/main/rust/sealevel/programs
- Sealevel client: https://github.com/hyperlane-xyz/hyperlane-monorepo/blob/main/rust/sealevel/client/src/main.rs
- Sealevel local e2e: https://github.com/hyperlane-xyz/hyperlane-monorepo/tree/main/rust/main/utils/run-locally/src/sealevel
- Hyperlane relayer / agent config: https://docs.hyperlane.xyz/docs/operate/config/config-reference
- Wormhole guardians: https://wormhole.com/docs/learn/infrastructure/guardians
- Wormhole NTT custom transceivers: https://docs.wormhole.com/Wormhole/native-token-transfers/custom-transceivers
- LayerZero Solana: https://docs.layerzero.network/v2/developers/solana/getting-started

Agent economy
- ERC-8183: https://eips.ethereum.org/EIPS/eip-8183 , https://ethereum-magicians.org/t/erc-8183-agentic-commerce/27902 , https://docs.rs/crate/erc8183/0.5.0
- Solana Agent Registry: https://solana.com/agent-registry/what-is-agent-registry
- Agent economy overview (8004/8183/x402): https://www.certik.com/blog/the-rise-of-the-agent-economy-part-1
- x402 facilitator on Solana: https://solana.com/docs/tools/x402-facilitator , https://solana.com/x402/what-is-x402
- x402 Rust crates: https://docs.rs/x402-chain-solana , https://docs.rs/crate/r402-svm/0.15.0

Local
- Pearl verifier entry point: research/pearlrepo/zk-pow/src/api/verify.rs (`verify_block`, plonky2/starky)
