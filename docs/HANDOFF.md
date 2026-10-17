# pearl-sol handoff

READ THIS FIRST in a new session. Update it after every step (decision, commit,
agent result). Keep it current, not historical: rewrite stale lines instead of
appending.

## Goal (owner, 2026-10-16, ongoing)

Build the Pearl-on-Solana design: spec first, then a working localnet M1. Owner
allows running as many agents as needed.

- Token chain: Solana, pump.fun launch, fixed 1B supply. A mining reserve is
  funded by the dev buy plus creator-fee buybacks.
- Agent chain: our OWN chain (Rust, Commonware primitives), running in parallel.
  Useful-PoW miners produce its blocks. Its native coin is the bridged token
  (lock on Solana, release from a genesis escrow; nothing minted).
- AI economy: agents post paid AI jobs and pay each other via native transaction types.
  Miners do Pearl cuPOW matmul on posted jobs; only proofs bound to a paid job
  earn from the reserve. Each job's mining reward stays below the fee it burns
  (stops fake-job self-dealing).

## Decisions

- PIVOT 2026-10-16 (owner): the agent chain is our OWN chain on Commonware
  primitives with useful-PoW block production. Not SVM, not Cosmos, not a fork.
  agent/ (Anchor) is superseded, reference only. Branch wip/svm-agent-chain holds
  the stopped SVM integration demo + extension WIP.
- Still valid: token chain = Solana (pump.fun, reserve, bridge program),
  crates/pearl-job, relayer/crank token side.
- Solana's role (owner approved 2026-10-16, both): (1) M1 compute storefront:
  a Solana program + x402 endpoint where any Solana app/agent buys AI jobs in
  USDC or SOL; payment swaps into the token (buy pressure), bridges to our chain
  as W + P, and the result commitment is posted back to Solana for the buyer.
  (2) M2 trust-minimised bridge: a Solana light-client program that tracks our
  PoW headers and verifies the work (plain Pearl verify, about 1.3-1.9M CU, split
  over 2 txs per header); bridge withdrawals and job results are proved against
  those headers instead of an M-of-N signer set. Plus the existing roles:
  liquidity (pump.fun/PumpSwap), creator-fee buybacks funding the reserve, miner
  exit liquidity. Framing: Solana = money, buyers, final record; our chain = work.
- Validators/builds run locally on this Mac (owner OK), not claude-host.
- Ports 10600-10699 (see CLAUDE.md). Repo is local git only, no remote.

## State

- [x] Repo, CLAUDE.md, research copied into research/ (commit f13cae4)
- [x] scripts/localnet.sh up|down|status: two validators, token :10600 and
      agent :10630, both verified (commit 9005000)
- [ ] Research agents (started 2026-10-16), writing into research/:
      NOTES-proof.md (DONE, summarised below),
      NOTES-pump.md (DONE, summarised below),
      NOTES-chain-bridge.md (DONE, summarised below).
      If a file is missing, that agent did not finish: rerun its topic.
- [x] docs/SPEC.md v1 (programs, bridge flow, emission, signed message formats, M1-M3)
- [x] Anchor workspaces scaffolded: token/ (reserve, bridge) and agent/ (escrow,
      agents, jobs, mining). Program keypairs + payer in keys/localnet/ (committed,
      localnet only). Anchor.toml points at :10600 / :10630 and the local payer.
- [ ] M1 localnet build: four build agents started 2026-10-16, each owning paths
      and committing only its own files (see CLAUDE.md "Who is working on what"):
      1. bridge: token/programs/bridge, agent/programs/escrow, services/relayer (:10660)
         DONE (9d23936, 5d0f61b): 10 bridge + 7 escrow + 8 relayer tests pass (LiteSVM).
         Escrow PDA GoszXkVtKwDgZSEWNjdPwuJ2eJGtohz6bCvnthNd5n5U, preload file
         services/relayer/fixtures/escrow-account.json. TODO at integration: add
         that --account to the agent validator in localnet.sh, then
         `node src/cli.ts init-bridge <mint>` and `init-escrow`. The invariant
         allows in-flight transfers. Orchestrator changed the default so a
         surplus (a donation) no longer halts the bridge; only a deficit refuses
         (ALLOW_SURPLUS=0 restores strict mode). Unlocks to a new account need
         amount_spl >= about 891 (rent). sigverify.rs is duplicated in both
         programs: keep them in sync.
      2. reserve: token/programs/reserve, services/crank, fixtures/pump, scripts/pump-fixtures.sh
         DONE (758ecd3, 869b3b7, 2e1b90e): 16/16 LiteSVM tests vs real pump bytecode,
         plus launch, curve buyback and fund_mining run on the real :10600 validator.
         Pump fixtures (4 MB, mainnet slot 457802307) committed and loaded by
         `scripts/localnet.sh up token`. TODO at integration: set the real
         agent-chain mining_pool PDA with `crank update --miningPool` (a
         placeholder was used). Deviations: separate crank_buyback (curve) and
         crank_buyback_amm (PumpSwap) instructions, a collect_curve_fees
         instruction, an optional config.cranker allowlist (a fully open crank
         can be sandwiched), new min_buy / sol_floor / max_fund_per_call fields,
         and max_slippage_bps defaulting to 300 (it must cover pump's 1.25% fee).
      3. pearl: crates/pearl-job (+ `pearl-job` CLI), services/miner, services/attester (:10661)
      4. market: agent/programs/{agents,jobs,mining}, services/agent-cli
         DONE (2286ea7): 8 unit + 18 LiteSVM tests pass; submit_share 31-43k CU.
         Deviations to fold into SPEC: poster picks created_slot (within
         max_slot_lag 150); alpha x P cap is per job lifetime, not per epoch;
         jobs has its own evaluator set (init sets both); permissionless
         advance_epoch; claim is per job; pool tracks `outstanding`; terms_hash
         = sha256("pearl-sol:terms:v1" || poster || job_id || m || n || k || W ||
         P || deadline_slot || share_target). agent-cli not yet run on a validator.
      Program tests run in LiteSVM (no ports). Not yet built: scripts/demo.sh, the
      real-validator end to end run (SPEC 5, M1 step 4); that is the orchestrator's
      integration step once all four report. If a session dies mid-build, check
      `git log` and each scope's files to see which workstreams landed.

- [x] Integration step 1: `scripts/localnet.sh up` now loads our built programs
      at genesis (reserve, bridge on token; escrow, agents, jobs, mining on agent;
      upgrade authority = localnet payer) and preloads the escrow with 1e18
      lamports. Verified: all 6 programs executable, escrow balance 1B. Both
      workspaces build (`anchor build` in token/ and agent/).

## Research results so far

- Chain/bridge (research/NOTES-chain-bridge.md, DONE):
  - Plain Agave cluster from our own genesis, own Agave fork from day one. The
    local solana-cli 3.1.12 has no solana-genesis/agave-validator (Anza stopped
    shipping them in v3.0): build from the fork for production. Localnet M1 uses
    solana-test-validator.
  - Gas must be native lamports, not SPL. Pattern: the whole 1B supply sits as
    lamports in a bridge escrow at genesis; deposit = unlock from escrow,
    withdraw = lock back. No minting. 1 SPL base unit (6 dp) = 1000 lamports.
    Genesis: --inflation none, --fee-burn-percentage 0. Invariant to watch:
    SPL locked on Solana x 1000 = lamport supply - escrow balance.
  - Pearl's verifier (zk-pow plonky2/starky) cannot run in a BPF program. M1:
    M-of-N attesters verify off chain and the program checks their signatures
    with the ed25519 precompile. Later: a native builtin or syscall in our fork
    (no feature gate needed on our own chain).
  - Bridge: M1 = own Anchor lock/unlock on both sides + relayer, 1-of-1 then
    2-of-3 signers. Production = self-hosted Hyperlane Sealevel (to verify: the
    route type and 6-to-9 decimal scaling).
  - Agent economy: port ERC-8183 (job escrow released by an evaluator; here the
    evaluator checks the proof) to Anchor; deploy the Solana Agent Registry
    (ERC-8004 port) for identity; x402 via a wrapped native token.

- pump.fun (research/NOTES-pump.md, DONE; repro in research/pump-litesvm):
  - Full flow proven against mainnet program bytecode in LiteSVM (in-process,
    no validator): create_v2, dev buy, buy, sell, permissionless fee collect,
    graduation, PumpSwap trades, post-graduation collect.
  - IDs: pump 6EF8rrecthR5Dkzon8Nwu78hRvfCKubJ14M5uBEwF6P, PumpSwap
    pAMMBay6oceH9fJKBRHGP5D4bD4sWpmSwMn52FMfXEA, pump_fees
    pfeeUxB6jkeY1Hxd7CsFCAjcbHA9rWtchMGdZ6VojVZ. IDLs from
    pump-fun/pump-public-docs @ 639b87f. Pump redeploys every few weeks.
  - create_v2 = Token-2022 mint, exactly 1B, mint and freeze authority null.
    Dev buy: 10% of supply is about 3.1 SOL, 20% about 7 SOL. Reserve size is
    the owner's call (suggested 5-10%).
  - Creator fees are SOL: 0.30% on the curve; tiered on PumpSwap (0.95% from
    420 SOL mcap down to 0.05% at 98k SOL). Collection is permissionless
    (sweep_creator_fee, then collect_creator_fee_v2 or collect_coin_creator_fee).
    Risk: pump admin can reassign the creator (community takeover).
  - Buyback design: creator = a data-less, system-owned program PDA. A
    permissionless crank runs sweep, collect, then buy (buy_exact_quote_in_v3
    on the curve, PumpSwap buy_exact_quote_in_v2 after). Tokens go to the
    reserve ATA. Needs a per-call cap and an on-chain min-out (sandwich risk).
  - Localnet: pump-rust-client's harness defaults to 8899/9000 (other projects'
    ports, do not run as-is). NOTES-pump.md has a solana-test-validator clone
    command for our 10600 block (written, not yet run).

- Proof (research/NOTES-proof.md, DONE):
  - Pearl ZK proof: 59.5 KB, verify 10-15 ms native, far over 1.4M CU on chain.
  - Plain (non-ZK) tile proof: about 13 KB at the smallest tile, 0.36 ms native,
    about 1.3-1.9M CU in SBF (split into 2 instructions, or add a BLAKE3 syscall
    in our fork for about 300-400k CU).
  - Job binding needs no circuit change: the 76-byte header is built from the
    job account; a miner-bound seed variant mixes in the miner pubkey.
  - CPU miner: zk-pow/src/ffi/mine.rs try_mine_one; difficulty 0x1d020000 is
    about 1 s per win on CPU. Pearl emission reference: 2.1B cap, C = 650,226.

- 3. pearl: DONE (f966a2d): 23 tests + 1 LiteSVM end to end against the real
     jobs/mining .so; proof 12.6 KB, verify about 95 us, share target 0x1d010000
     about 1 s per share per core. result_hash = blake3_keyed(C as i32 LE
     row-major, key = job_key). Share message has no tile (one share per nonce).
- All four M1 workstreams DONE. Left: the real-validator end to end run.
- Validators run locally (owner OK, see CLAUDE.md), not on claude-host.

- [ ] Chrome extension wallet (owner request 2026-10-16): build agent started,
      scope extension/ only. MV3 + React; one ed25519 key for both chains (base58
      or mnemonic import, encrypted at rest); balances, send, bridge in/out with
      pending tracking; agent register, browse/post jobs (in-browser Pearl
      commitments must byte-match pearl-job, else browse-only), mining claim;
      Wallet Standard provider for dApps with approval popups. Geist, no mono,
      no logo, name in one APP_NAME constant ("Pearl Wallet" placeholder). UI
      checks in headless Chromium only.

## Next step

SPEC v2 written (28a685f). Build agents started 2026-10-16 (max 3 heavy builds at
once on this Mac), each owning its paths:
1. node: chain/ workspace (agent-types, state, executor, consensus-pow, sync, net,
   mempool, rpc, bin/agent-node), chain/rust-toolchain.toml (Rust >= 1.95,
   Commonware 2026.9.0 pinned), docs/RPC.md (written early: miner, relayer and
   wallet build against it), scripts/devnet.sh (3 nodes, RPC 10630/10632/10634,
   p2p 10640-10649). Tx/header test vectors in chain/agent-types/vectors.
2. miner: DONE (13f27ae..8e7cdc7): pearl-job `pow` (verify_block = the node's full
   check; fallback derivation in PROOF-FORMAT.md section 7), new crates/pearl-node
   (JSON-RPC client per docs/RPC.md, own tx encoding byte-checked against
   chain/agent-types/vectors, MockNode), services/miner block producer with
   --deliver, attester node mode (raw ed25519 result sigs). 42 tests pass. Only run
   against the mock node so far. --deliver needs the payout funded (rewards mature
   after K). About 3k nonces/s on 8 threads under load.
   Attester/evaluator/deposit messages are RAW ed25519 over the domain-prefixed
   bytes; only tx signatures use Commonware framing (node agent, fef1490).
3. storefront: DONE (18f3cd0): 13 program + 10 service tests (LiteSVM, real pump +
   bridge). buy_job 173k CU, buy_job_amm 196k, finalize 13k. As-built details in
   SPEC 5.1; chain-side order rules sent to the node agent. Not done: USDC buys,
   relayer order support, an automatic finalize submitter, a real-validator run.
   Note: sigverify.rs now has 3 copies (bridge, escrow (superseded), storefront).
After those: rewire services/relayer to the node RPC (K-deep), rebuild the wallet
extension for our chain (tx encoding SPEC 2, vectors from agent-types), then
scripts/demo.sh (SPEC 7 M1 item 6). If a session dies, check git log per path.

## Gotchas

- ~/memechain and ~/ansemchain are deleted locally. The private backup
  cocainebit/ansemchain needs `gh auth login` (token invalid 2026-10-16). Not
  needed for SVM.
- No GPU on this Mac: a localnet miner must use a CPU path at low difficulty.
- research/pearlrepo is gitignored (51 MB clone of pearl-research-labs/pearl @ 837c69c).
