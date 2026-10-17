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
      Program tests run in LiteSVM (no ports). The v2 end to end run is scripts/demo.sh
      (see Demo below). If a session dies mid-build, check `git log` and each
      scope's files to see which workstreams landed.

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
- All four M1 workstreams DONE; the v2 end to end demo passes (see Demo).
- Validators run locally (owner OK, see CLAUDE.md), not on claude-host.

## Next step

M1 COMPLETE (2026-10-17). M2 COMPLETE (2026-10-17, wrap-up item 5 below; all tests
and the demo pass). Agents, each
owning its paths, max 3 heavy builds at once (all local; claude-host is destroyed):
1. light client: DONE (f5caf0f..50bf84c). See SPEC 5.3 "As built". Tests: verifier 7,
   LiteSVM 9, real devnet fixture 1 (15 real headers incl. the first live job
   block + a real withdrawal released by MMR proof); bridge 10/10 and storefront
   13/13 re-run. header-relay ran live (18 headers, resume OK). Now in scripts/demo.sh
   (item 5); reorgs capped at about 30 headers per tx.
   SECURITY: resolved (82e002f..1d4dc89). Pearl's rank penalty ported everywhere
   (pearl-job, chain via verify_block, pearl-verify-sbf + lightclient); r >= 128,
   default config now k=2048, r=128, 4x8. Zero/low-rank matrices only give a 1.22x
   edge (attack = 0.82x honest work), same as Pearl mainnet; SPEC 2/3.2 + 8.
   Light-client cost doubled with k: job header 7.25M CU / 34 txs, 4x8 fallback
   10.2M / 37 txs. Real-devnet fixture re-harvested at k=2048 (item 5). Still on the OLD config (refused by the
   chain now): extension LOCALNET_CONFIG_HEX + mock-rpc + solana.test,
   services/agent-cli:422, token/tests/storefront test:102 (sent to the DA agent).
2. open net: DONE (0eccee0, 645640a, a557c24): chain/net/src/open on
   commonware-stream (any key admitted under limits; plaintext key banner then
   encrypted handshake; genesis-bound namespace; `key@host:port` pins a seed),
   scored address book with gossip + persistence (<data_dir>/p2p-addrs.json),
   outbound subnet diversity, inbound/IP/subnet/pending caps, ban scores; node
   blocks peers that gossip invalid blocks. `agent-node run --net open --seeds`.
   Static mode stays the default (demo unchanged). `scripts/devnet.sh open ...` = 5
   nodes (.devnet-open, RPC 10630-10638 even, p2p 10640-10644). 19 new tests; live:
   full mesh in 8 s, miner 15 blocks, seed down + restart rejoin via saved book.
   Risks/not done: local addresses exempt from IP limits (allow_private), no
   hashed buckets/anchors/feelers, no getPeers RPC, untested across real hosts/NAT,
   node.json open fields not yet in docs.
3. pools: DONE (810ad1b, 5e91163): services/pool `pearl-pool` (:10663; routes in
   src/client.rs), PPLNS by share work, fee bps, credits mature after K+1, orphans
   dropped, payouts signed + persisted before send (no double pay on crash);
   miner `--pool URL`. 12 new tests. Live: 304 blocks, 0 orphaned, 84 payouts
   confirmed, on-chain balances equal the pool ledger exactly; restart mid-run OK.
   Not done: push notifications (about 18% stale shares on the fast devnet),
   vardiff, signed miner requests, live job-block run, payout history pruning.
   Devnet fee helper: services/pool/examples/devnet_fees.rs.
4. data availability + leftovers: DONE (b717103..52303a6). chain/blobs: blake3-addressed
   blob store, putBlob/getBlob/hasBlob/getBlobStats, one-hop push + fetch on demand,
   retained while a job is open/delivered + grace_blocks (100), `blob:<hex>` URIs
   (job data format PSD1). Min protocol fee (getParams.min_protocol_fee_effective),
   getBlock fees/subsidy/reward/matured, getAccountTxs, getPeers, docs/NODE.md.
   Fallback default 4x8 (verify 0.14-0.19 ms). Miner/attester/extension use blobs.
   Tests: chain 66, crates 67, extension 58 + 47 UI, storefront 13; demo 7/7 PASS
   (74.6 s) with job data as a blob. Live 5-node open devnet: cross-node blob fetch,
   late node catch-up (bug fixed ba4b0ce), pruning. Existing .devnet dirs need reset.
5. WRAP-UP: DONE (42e7958..). Tests: relayer 28 (+4), pool 9 (+1), light-client
   LiteSVM 9 + devnet fixture 2 (0 ignored), miner 13 unchanged; demo 8/8 PASS.
   - relayer: before an order BridgeUnlock it compares the order's P with
     getParams.min_protocol_fee_effective; below it nothing is signed, the deposit is
     held in state.needsRefund (reason, buyer, recipient, P, floor), logged once,
     shown in tick `needsRefund` and `cli.ts status`, re-checked every tick (lowering
     the floor unlocks it). Orders no longer Pending on Solana are held the same way.
     REFUND GAP (not invented): such a deposit has no way back. Tokens stay in the
     bridge vault (invariant counts them in flight); storefront expire_order only sets
     status Expired (moves nothing); expire_order_proof needs a chain ExpireJob that
     cannot exist (the job was never posted); the bridge has no instruction returning
     a vault deposit on Solana. Options for the owner: a bridge `refund_deposit`
     (signers over nonce + buyer) or a plain BridgeUnlock of W + P to the recipient.
     Relayer also reads bridge release_modes / storefront finalize_modes and leaves
     final withdrawals / order results to the proof path when signers are off
     (`proofMode`).
   - light client: fixture re-harvested from a 1-node devnet at k2048/r128, 4x8, K 6
     (17 headers, 1 job block, 2 withdrawals; withdrawal 0 released by proof, 21.6k CU);
     replay test un-ignored; old fixture kept as devnet-pre-rank-penalty.json for the
     rejection test. Bug fixed: header-relay `seed` posted `blob:seed`, refused by the
     node; it now putBlobs the PSD1 data.
   - header-relay: `run --settle` (src/settle.rs) submits bridge release_proof and
     storefront finalize/expire_order_proof; `status` subcommand; 50 ms confirmation
     polling + batched proof chunks: about 1.4 s per header on 50 ms slots (was about
     20 s). GAP: getWithdrawalProof proves only against the node's current final
     withdraw_root, so the light client needs a final header carrying that root (any
     header after the last withdrawal block); with withdrawals every block it must sit
     at the node's final tip. Fix: a historical-proof RPC (Commonware keyless
     historical_proof at the block's log size).
   - demo: light client + proof mode (see Demo); scripts/localnet.sh loads
     lightclient.so; demo token validator runs --ticks-per-slot 8.
   - pool: default share factor 16 -> 11 (pearl_pool::DEFAULT_SHARE_FACTOR). Live:
     64x64 era 13.6 pool shares per chain block; 4x8 at 16: 14.6-16.9; at 11: 13.4.
     Test: 4000 nonces at 4x8 give 11.1 shares per block.
6. AGENT CREDIT LAYER (owner 2026-10-17, started): wire pearl-sol's verified work into
   ~/agent-credit ("Tab", Solana adaptation of priors.trade): verified jobs/mining
   feed the credit passport, loans fund jobs, mining-backed lines. Work happens in
   ~/agent-credit (design ~/ideas/agent-credit/PEARL-LAYER.md); pearl-sol only
   receives precise change requests. Priors itself (Robinhood Chain) is NOT
   targeted (third-party; would need a new RH Chain bridge).
7. AGENT WALLET = LEATHER FORK (owner 2026-10-17): ~/pearl-wallet (Leather 6.73.4,
   MIT, remote `upstream`, no remote of ours). Our chain is the primary network +
   Solana token side; ports our extension's byte-tested codecs; Bitcoin/Stacks
   hidden behind one flag (default off); Leather branding removed. Plan doc
   ~/pearl-wallet/docs/PEARL-PORT.md. The simpler ~/pearl-sol/extension stays as the
   reference implementation until the fork matches it.
Not done in M2 (later): order refund path (gap above), historical withdrawal proofs,
chunked blob manifests, persistent history index, multi-hop
blob push, getPeers across real hosts, USDC storefront, demo coverage of
PumpSwap/expiry/reorgs/restarts. agent-cli selftest keeps the old k on purpose
(mirrors the superseded agent/ terms_hash vector; never submitted).
Owner decisions still open: dev buy size, chain params (block time, K, alpha,
emission), name and ticker. If a session dies, check git log per path and
CLAUDE.md "Who is working on what".

## M1 components (SPEC v2, all DONE)

1. node: DONE (5257d6e..d09a1b6): 9 crates in chain/, Commonware 2026.9.0, Rust 1.95.0
   pinned. 34 tests (incl. deterministic multi-node: fork race, partition/heal,
   refusal beyond K, late joiner sync, withdrawal proof). 3-node devnet via
   scripts/devnet.sh (init/start/stop/status/mine/reset), RPC 10630/10632/10634, WS
   = RPC+1. Measured: 51 blocks/s at easy target, about 480 tx/s end to end; live
   pearl-miner 3/3 blocks accepted. Deviations: chain job header =
   header_id||terms_hash||0||0 (header_id = job_id for PostJob, = poster for storefront
   orders with order_terms, see docs/RPC.md; M1 integration fix); genesis tx_root = genesis spec digest; BridgeUnlock fee
   may be 0; work = 2^256/(target+1); withdrawal proofs = Commonware MMR (SHA-256),
   final only. Not done: open-membership net, body pruning, restart loads all
   blocks in memory, no job block mined live yet (needs a job with real data_uri).
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
4. relayer: DONE (adedade): 24 tests (vectors + LiteSVM with real bridge/storefront
   .so + in-process fake node with reorgs). Plain and order BridgeUnlock, releases
   only when final and >= K deep, finalize/expire orders if it holds attester keys
   (else listed in needsAttester), invariant via getEscrow. Env: NODE_RPC,
   AGENT_KEY, STATE_FILE (.state/relayer.json). Not run against the real node yet
   (its RPC was still a stub). One relayer per AGENT_KEY (nonce contention).
5. wallet extension: DONE (00b6d73..75b2fee): extension/, 56 unit tests + 45 headless
   UI checks (mock), and LIVE on the demo stack: 37/37 read-only smoke + 9/9 writes
   (bridge-in, PostJob, storefront buy; both jobs mined with wallet-computed
   commitments). Storefront Buy computes hash_a/hash_b itself (order_terms; vector
   chain/state/vectors/order_terms.json). Load unpacked per extension/README.md;
   live writes: extension/scripts/live-writes.mjs. Remaining: mining view shows
   fees only (getBlock has no subsidy field); chain activity is the wallet's own log;
   token label PEARL and APP_NAME are placeholders.
6. M1 demo: DONE. `scripts/demo.sh` passes end to end on real local processes
   (all 7 steps PASS, about 3.5 min plus builds; details in "Demo" below).

## Demo (scripts/demo.sh, SPEC 7 M1 item 6)

Run: `scripts/demo.sh` (builds agent-node and pearl-job/miner/attester if stale, refuses
if any demo port is bound, wipes and recreates `.demo/`, runs `scripts/demo/run.ts`,
tears everything down by PID, exit 1 on any FAIL). `DEMO_KEEP_UP=1 scripts/demo.sh`
leaves the stack up (then `scripts/demo.sh down`). Logs: `.demo/logs/*.log`, results
`.demo/result.json`, demo keys `.demo/keys/` (throwaway, regenerated each run).

Topology: token chain `scripts/localnet.sh up token` (:10600); agent chain
`scripts/devnet.sh` with `DEVNET_DIR=.demo/devnet` (3 nodes, RPC 10630/10632/10634, p2p
10640-10642); attester :10661 (node mode), miner (`--deliver`, 4 threads, payout
keys/localnet/miner), relayer :10660 (SIGNER_KEYS = attester, AGENT_KEY = TOKEN_PAYER =
payer, POLL_MS 1000), storefront-x402 :10662 (wallet = a demo key), crank CLI,
header-relay (`run --settle`, payer = keys/localnet/payer = storefront + bridge admin).
The token validator runs `--ticks-per-slot 8` (DEMO_TICKS_PER_SLOT) so the relay keeps
pace with the agent chain.
One key (keys/localnet/attester) is the chain attester (genesis, 1 of 1), the bridge
signer (1 of 1), the storefront attester (1 of 1) and the job evaluator.

Demo genesis (DEVNET_PARAMS): finality_k 6, target_block_time_ms 1500,
wtema_half_life_ms 15000, emission_ppm 10000, min_fee 1000, target 0x207fffff (the
easiest; WTEMA hardens it within about 200 blocks). emission is high on purpose so one
job block pays exactly alpha x P (cap binds). Production values stay K 30 / 60 s / 7 ppm.

Steps and last run (2026-10-17, all 8 PASS, 122.8 s without builds; was 74.6 s before the
light client):
1. (13.3 s) up + init: reserve init, launch (1 SOL dev buy), bridge init (attester 1 of 1),
   storefront init + trader volume accumulators, genesis attester checked.
2. (15.6 s) trades: 0.0306 SOL creator fee; crank buyback +847.9k tokens; fund 10M tokens
   unlocked to the pool.
3. (16.3 s) user deposit 8.69M tokens unlocked; RegisterAgent; miner payout funded.
4. (6.9 s) light client: `header-relay init` at the node's final header (height 217),
   `header-relay run --settle` started, storefront `set_finalize_modes` proof only;
   light client follows (tip 220).
5. (8.7 s) `pearl-job gen` 64x64x2048 r128, job data blob on node 1, x402 order -> relayer
   BridgeUnlock(job_order); getJob matches the buyer's commitments.
6. (24.2 s) 1 job block (subsidy = alpha x P), CompleteJob at 230; Order finalized on
   Solana by `finalize_order_proof` (asserted from the tx) against final light-client
   header 230; result_hash = the chain's.
7. (33.5 s) bridge `set_release_modes` proof only; miner BridgeLocks half its balance at
   251; `release_proof` against final header 251 (asserted from the Processed PDA's tx);
   SPL checked; relayer signed nothing (reported proofMode).
8. (4.2 s) teardown by PID incl. header-relay, ports 10600-10662 free.
Final invariant: vault 18,836,394,149,842 SPL x 1000 = 1e18 - escrow, nothing in flight,
delta 0. header-relay verified 40 headers in the run.

Bugs fixed by the integration run (each with a test):
- Storefront orders were unmineable: the Pearl job header (job_id || terms_hash with W,
  P and the absolute deadline) cannot be known when the buyer commits hash_a/hash_b on
  Solana. Order jobs now use `poster || order_terms(poster, m, n, k, max_blocks,
  relative deadline)` (chain/state schema `order_terms_hash`, JobRecord.header_id, RPC
  `header_id`). The extension's Buy form can now compute hash_a/hash_b from this rule.
- pearl-job pow refused job blocks whose chain job_id differs from the header id;
  pearl-node JobInfo gained header_id; miner delivered under the wrong id; the miner
  stalled the chain retrying a job whose mining errors (now skipped 60 s).

Not covered by the demo: PumpSwap (graduated) path, expire/refund path (and the
below-floor refund gap), USDC, reorgs
across real nodes, restarts mid-flow, more than one relayer/attester.

## Gotchas

- ~/memechain and ~/ansemchain are deleted locally. The private backup
  cocainebit/ansemchain needs `gh auth login` (token invalid 2026-10-16). Not
  needed for SVM.
- No GPU on this Mac: a localnet miner must use a CPU path at low difficulty.
- research/pearlrepo is gitignored (51 MB clone of pearl-research-labs/pearl @ 837c69c).

- DISK: the Mac's disk filled on 2026-10-17 (ENOSPC stops every tool, including the
  orchestrator's). Agents check `df -h /System/Volumes/Data` before heavy builds and
  stop under 6 GB. Throwaway test-crate target dirs (token/tests/*/target,
  crates/pearl-verify-sbf/target, crates/chain-litesvm/target) are safe to delete;
  never chain/target casually (18 min rebuild) or token/target/deploy.
