# pearl-sol handoff

> 2026-10-20: session handoff (account switch) in docs/SESSION-HANDOFF.md; read it first.

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
     Refund path: CLOSED by item 8 (bridge refund_deposit + storefront mark_refunded).
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
     20 s). The current-root gap is CLOSED by item 8 (historical proofs).
   - demo: light client + proof mode (see Demo); scripts/localnet.sh loads
     lightclient.so; demo token validator runs --ticks-per-slot 8.
   - pool: default share factor 16 -> 11 (pearl_pool::DEFAULT_SHARE_FACTOR). Live:
     64x64 era 13.6 pool shares per chain block; 4x8 at 16: 14.6-16.9; at 11: 13.4.
     Test: 4000 nonces at 4x8 give 11.1 shares per block.
6. AGENT CREDIT LAYER: built + pushed in ~/agent-credit (ac_pearl, 4a179dc..32f3b67;
   design ~/ideas/agent-credit/PEARL-LAYER.md, AC-D32..42). Owner 2026-10-17:
   pearl lines require a backer stake (AC-D39, agent started in ~/agent-credit);
   pearl-sol built requests R1, R2, R5 (item 9). R3/R4/R6 not requested yet.
7. AGENT WALLET = LEATHER FORK: DONE on ~/pearl-wallet branch `dev` (e07b9a3..ae3dce1,
   not pushed; no remote of ours). Opens on our chain + Solana side; Leather's
   phrase derives ed25519 m/44'/501'/i'/0'; base58 import; window.pearl + Solana
   Wallet Standard with approvals; BTC/STX behind ENABLE_BITCOIN_STACKS (off);
   telemetry off; Leather branding removed (LICENSE kept). Tests: 187 unit, 63 UI
   (mock), 16/16 live on the demo stack. Write-up docs/PEARL-PORT.md. Follow-up agent
   REVAMP (owner 2026-10-17, supersedes the lien-screen follow-up, which was
   stopped): Bitcoin/Stacks REMOVED entirely (code + deps, no flag); all our
   systems wired into Leather's EXISTING screens with minimal UI change (assets,
   send/receive/activity, swap screen = bridge + storefront, networks, dApp
   approvals = window.pearl + Solana Wallet Standard; jobs/mining/liens/Agent
   Credit built from Leather components). DONE (dev 2627b50..4f1c4b5, not pushed):
   src 65k -> 19k lines, deps 133 -> 50, dist 21 -> 10 MB; old vaults still unlock
   (cipher reimplemented byte for byte). Unit 103, UI 56/56, live 21/21. Gaps:
   Agent Credit page only tested against a mock (ac_pearl not deployed, its API not
   in the demo), no fiat prices, chain activity grouped by height, no password
   change, Leather CI workflows still in repo, Firefox untested.
   R7/R8 DONE (0a61d4d); Agent Credit keeper auto-release DONE (pushed
   b9c967d..255715c).
   Agent Credit (2026-10-17): R1/R2/R5 integrated + pushed (ce5410f..b9c967d);
   owner chose release key = Agent Credit keeper key; follow-up agent builds R7/R8
   here + keeper auto-ClearRoute on repayment there.
8. REFUND + HISTORICAL PROOFS: DONE (6743aa0..c3d1bc0; SPEC 5.1 "Refunds", 5.3
   "Historical withdrawal proofs"). Owner decision 2026-10-17: refund held-back orders
   on Solana, in tokens.
   - bridge `refund_deposit(nonce, buyer)`: bridge signers over `pearl-sol:refund:v1 ||
     nonce || buyer || amount_spl`; full Deposit amount from the vault to the buyer;
     refund record `Processed ["processed", [0], nonce]` (replay guard; withdrawals keep
     [1]); open in any release_modes. About 23k CU.
   - storefront `mark_refunded()`: permissionless, CPI-free read of the refund record
     (owner, address, nonce, recipient = Order.buyer, amount = Order.tokens); Pending or
     Expired -> Refunded (status 4). About 10.5k CU.
   - relayer: held-back orders refunded after REFUND_GRACE_MS (default 1 h, -1 never)
     in one tx (ATA, ed25519, refund_deposit, mark_refunded). Persisted before signing:
     `signedUnlock` (never refunded) and `refunds` (never unlocked); refund records are
     read with the vault, so refunded deposits are not pending, the invariant is exact
     and the cursor passes them; never refunds a deposit the chain processed. Mutual
     exclusion across operators rests on the signers (bridge signers = chain attesters,
     threshold > N/2), written in SPEC 5.1.
   - chain: getWithdrawalProof `at_height` (keyless historical_proof at that final
     block's log size; per-height log-size index rebuilt on restart). header-relay
     `--settle` proves at the light client's final height (hash checked; current-root
     fallback); `seed` locks twice and `harvest` adds `withdrawals_at_own_block`.
   - Tests: bridge LiteSVM 13 (+3), storefront 15 (+2), relayer 33 (+5: refund after
     grace, never after a signed unlock, never after a chain unlock, a refund signed
     elsewhere stops the unlock, grace -1 / no keys), chain 69 (+3: store historical
     proofs verified with pearl-verify-sbf at every commit boundary incl. multi-block
     finalize, restart, node RPC), light-client LiteSVM 9 + fixture 2 (fixture
     re-harvested: 13 headers, 1 job block, withdrawal 0 released by a historical proof
     against an older final header where the current-root proof fails).
   - Demo 9/9 PASS (132.8 s): new step 8 (see Demo).
   - Remaining: plain (non-order) deposits are never auto-refunded; the cross-operator
     guarantee is a signer rule, not enforced on chain; refunds return tokens, not SOL.
9. AGENT CREDIT R1 + R2 + R5: DONE (83fc268..67d76b5; SPEC 3.4 "Coinbase receipt", 4.4,
   5.3 "Final-header accumulator"; RPC getRoute, getBlock coinbase + paths).
   - R1: every non-genesis block's tx_root ends with a 97-byte coinbase receipt
     (`0x00 || height || miner || fees || subsidy || job_id || mature_height`; the 2
     reserved bytes of the draft dropped), carried in the block body (context-free
     tx_root check) and required to equal execution's. getBlock returns it with its
     path (and every tx's path); `pearl_verify_sbf::chaintx::verify_coinbase`.
   - R2: SetRoute (kind 9) / ClearRoute (kind 10). One lien per account, extension-only
     while active, release key over `pearl-sol:route-clear:v1 || chain_id || account ||
     route_nonce`; splits matured coinbase, CompleteJob W, ExpireJob refunds into
     withdrawal entries (from = account) to the Solana recipient; < 1000-unit carry in
     `pending`; held while the bridge is paused; dust back when the lien ends. Wallet
     codecs in extension/src/chain (tx.ts, messages.ts, rpc.ts).
   - R5: light-client `Accumulator ["acc"]` (SHA-256 MMR of final headers):
     init_accumulator / accumulate / prove_header / restore_header, close_header waits
     for accumulation; header-relay accumulates and has `acc-proof H [--restore]`.
     CU: prove_header 17.0-20.8k at 2^20-1 leaves (short proof 608 bytes), restore 25.9k,
     accumulate about 2.7k per header.
   - Store fix (5f59002, orchestrator's side finding): Commonware batches hold only a
     Weak parent, so a caller keeping only the tip snapshot lost uncommitted ancestors
     ("left child missing" / "journal error: item out of range"). Snapshots now keep
     their uncommitted ancestors alive (at most about K). The chain itself never hit it
     (it keeps every non-final snapshot in `live`); 768 + 400 finalize schedules, a
     consensus K 6 -> 1 finality jump with withdrawals, restart: all pass.
   - Tests: chain 81 (+12), crates 72, pearl-verify-sbf 12 (+3), light-client LiteSVM 11
     (+2) + fixture 2 (re-harvested with receipts; accumulate, close and restore the job
     block's header, then prove its coinbase reward), relayer 33, extension 63 (+7).
     Demo 10/10 PASS (197.2 s) with new step 9 (see Demo).
   - For Agent Credit: see the notes in the R1/R2/R5 report (also SPEC 3.4/4.4/5.3).
   - R7 + R8 (2026-10-17): getAccountTxs lists ClearRoutes naming the account (role
     `lien`, also when a third party sent them); getRoute adds `set_block` (block hash,
     height, index of the SetRoute behind the record) and `last_clear` (newest canonical
     ClearRoute naming the account, with its route_nonce, or null). Node-local index
     (consensus-pow `clears`, extended at finalization, live part read from the branch),
     no state or encoding change. Chain 82 (+1).
     Existing light clients need `init_accumulator` (admin) before `close_header` works
     again; existing .devnet data must be reset (block encoding changed).
10. PEARL CLI / TERMINAL DASHBOARD: DONE (947b5c5, 8e1a3ce, inline restyle see below; docs/CLI.md). `crates/pearl-cli`,
   binary `pearl` (own workspace, Rust 1.95 pinned; excluded from crates/). Retro blue ratatui
   dashboard after the owner's "Reloaded Finance" reference (block wordmark, red beta banner,
   slash-titled panels, green-selected table, black shell strip with a real command line and
   hotkeys) plus the same commands in plain mode with `--json` for agents. Name is a
   placeholder: NAME / TICKER / wordmark all come from `src/brand.rs` (one-line rename; the
   crate and binary names change with it). Every figure is an RPC read; no price source, so
   none shown (/Market = storefront min job price, realised fill, "no price source").
   Reuses agent-types (tx encode/sign), pearl-verify-sbf (PostJob job id/terms), pearl-job
   (data, commitments), pearl-node (RPC, key files); Solana deposit is a raw legacy tx.
   Tests: 21 unit + TestBackend render. Live on the demo stack: balance, send, 5 job posts
   (all mined + completed), bridge in (relayer unlock) and out (released by proof), register,
   lien set/clear, pty-driven dashboard incl. mine start/stop. Screenshots
   screenshots/cli/live-*.png. Gaps: Agent Credit only as a reachability probe (`--credit-url`),
   no price source by design, pool panel only reads /stats, invariant in-flight scan covers the
   last 64 nonces each way.
   INLINE RESTYLE (2026-10-17, owner's Daytona-CLI reference): text mode is now plain inline
   output: one `✓` step per real event (signed, submitted, included in block N, K-deep final,
   Solana deposit confirmed, relayer unlock, withdrawal released, job mined / delivered /
   completed, blocks found by `mine start`), spinner on the step in progress, red `✗` with the
   error, green progress bar + elapsed time for waits, and every command ends in a gray
   rounded box with a white title chip (dim labels, white values, colored states). `pearl`
   with no command prints an overview box + hint line; the dashboard moved to
   `pearl dashboard` (alias `tui`, unchanged). Text mode follows writes to the end
   (`--no-wait` to stop at submit); `--json` output and wait behavior unchanged. NO_COLOR or
   no TTY: no colors or spinners. Renderers in `src/term.rs` (hand-rolled ANSI, no new deps).
   Tests 35 (renderers with / without color, every command's text + JSON path against a
   canned loopback JSON-RPC node). Live on the running demo stack (not restarted): overview,
   balance, send, job post to completion, job show by prefix, bridge out (released by the
   proof path, 5m 43s), bridge in, bridge status, lien show, status, jobs. Real ANSI
   screenshots: screenshots/cli/inline-*.png (crates/pearl-cli/tools/inline-screens.sh).
   Gaps: the job lifecycle bar is 3 stages (mined / delivered / completed), unlock and
   release are 0/1 bars (no finer real measure); `mine start` Ctrl-C stops the miner and the
   CLI together, so its summary box shows only when the miner exits on its own; register and
   lien set / clear were not re-run live (user account in use; covered by the stub test path
   for the tx box, and by the earlier live run of the same tx code).
11. TEST VPS: DONE 2026-10-18. Owner's BitLaunch box pearl-sol-test (64.95.13.106, user pearl,
   $0.156/hr) runs the full stack; demo 10/10 PASS (468 s) there with no Linux fixes, and a
   DEMO_KEEP_UP=1 stack left up. Sync, start/stop, tunnel (local 206xx): docs/VPS.md,
   scripts/vps.sh sync|ssh|tunnel|demo|down.
11. TEST SWEEP (2026-10-18): every suite run, matrix in docs/TEST-REPORT.md. pearl-sol 428
   tests pass, 0 fail, 1 ignored (miner bench); 8 SBF programs build. Fixed: chain-litesvm e2e
   and agent-cli selftest --pearl-job still used k 1024 (a113437, a3d85a9); miner HTTP test
   port moved to 10690-10699 (96131e0). OPEN: token/target/deploy bridge.so + storefront.so
   are older than their source (fresh builds pass every suite; rebuild into target/deploy and
   re-pin ~/agent-credit checks/m55 at the next demo restart): DONE in item 13. Demo e2e: being run on the VPS.
12. EXPLORER ("every block was paid for", SYNTHESIS item #1): DONE 2026-10-18. apps/explorer
   (no deps, Node 22 built-ins; :10670), docs/EXPLORER.md. Per block: job mined or FALLBACK,
   who paid (storefront Order + buy tx + x402 payment, or PostJob), W, P, subsidy and the job's
   cumulative subsidy vs alpha x P, proof hash + raw proof, Solana light-client verdict and its
   txs, settlement mode (finalize_order_proof / release_proof vs signers). Totals: paid vs
   fallback share, SOL settled, P (into the mining pool: nothing is burned on the chain, the
   UI says so), subsidy vs P, unique payers + top-payer share, buyback panel from the
   storefront's bb_* counters and BuybackExecuted events (hidden until the deployed program has
   them). JSON twin for every page under /api. 20 tests (math, decoders/PDAs, indexer reorg +
   genesis reset + persistence + routes on a fake node/Solana). Live on the test VPS via the
   tunnel: 8 of ~24,700 blocks on paid jobs; 3 x402 orders bought with tools/buy-jobs.mjs and
   captured live. Screenshots screenshots/explorer/. Findings: local Solana keeps ~75 s of
   history, so the indexer records signatures as it sees them (older ones "not recorded"); the
   box's header-relay was ~12,000 blocks behind (fixed 2026-10-18, item 14), so new Orders
   stayed pending there.
13. STOREFRONT BUYBACK (SYNTHESIS item #2): DONE 2026-10-18 (commits 3afa7c5..; SPEC 5.1
   "Buyback"). Before: all of x402 `sol_in` bought W + P, the `overhead` only paid rents and
   fees, so there was no margin. Now `buy_job*` pays `buyback_bps` of `sol_in` into the
   buyback vault PDA `["buyback"]` (an on-chain rule; the x402 service grosses `sol_in` up so
   the job keeps its price) and a permissionless `crank_buyback` / `crank_buyback_amm` buys
   the token through the reserve's pump path (curve, PumpSwap after graduation) and sends
   it to the mining reserve ATA or burns it. `set_buyback` (admin) edits share,
   destination, reserve program, cranker allowlist, cap, min buy, interval
   (`BuybackConfigUpdated`); every buy emits `BuybackExecuted` (SOL in, tokens out,
   destination, order nonces covered, carry), every order `BuybackAccrued`. Bridge invariant
   unaffected; after a burn the bound is `1e18 - escrow <= 1000 x mint supply`.
   Surfaces: storefront-x402 quote fields + `GET /buyback`; crank `sf-status | sf-setup |
   sf-set | sf-buyback` (and `run`/`once` crank it); `pearl buyback [--json]`; extension/
   and ~/pearl-wallet (ae2c36d) pass the new buy_job account and quote on sol_in - share;
   apps/explorer already decodes the same layout.
   Tests: storefront LiteSVM 21 (+8 buyback), storefront-x402 11 (+1), reserve 16,
   relayer 33, bridge LiteSVM 13, light client 11 + fixture 2, pearl-cli 36 (+1),
   extension 63, pearl-wallet unit 103 + `pnpm checks`. Demo on the VPS: 11/11 PASS (1138 s, 2026-10-18,
   restarts at 14:31, 14:34 and 14:36 UTC; stack left DOWN). New step 6: order 2 paid
   1,112,043 lamports (1000 bps of sol_in) into the vault; buy 1 556,021 lamports -> 15.82M
   SPL to the mining reserve (orders 2..2, carry 556,022), buy 2 the carry -> 15.82M SPL
   burned (mint supply 1,000,000,000 -> 999,984,178.8 tokens); totals and GET /buyback
   agree; bridge vault unchanged; final invariant OK, delta 0. The demo sets min interval 0
   and min buy 0.0001 SOL (the demo order is small).
   Builds (VPS, same program ids, keypairs unchanged): storefront.so `a8e5ef65...ac80`,
   bridge.so `1ca30604...0a39` (rebuilt from current source; was stale `599467df...`),
   both copied to token/target/deploy + IDLs; ~/agent-credit m55 re-pinned and PASS (pushed
   091dd57). Gotcha: LiteSVM's node binding aborts (std::bad_alloc) on the VPS, so the JS
   LiteSVM suites ran on the Mac.
   OWNER DECISIONS 2026-10-18: share 10% (1000 bps) and destination mining reserve are
   DECIDED; a refunded order also refunds its share (built, below). Still PROVISIONAL / open:
   per-call cap (1 SOL), min buy (0.001 SOL), interval (600 s), whether to allowlist the
   cranker (sandwich risk as for the reserve's buyback).
   SHARE REFUND (follow-up, 2026-10-18, f9b9f44..): each order's share is HELD in a
   `ShareRecord ["share", nonce]` (new last account of buy_job*, after the vault) and counted
   in `bb_locked`; cranks spend only `vault - rent - bb_locked`. Permissionless
   `settle_share` once the order closes: Finalized -> released to the buyback; Refunded or
   Expired -> back to the buyer from the vault; record closed (no double settle). Chosen over
   a refundable reserve because the vault then always covers every unsettled share. The
   relayer's refund tx now also calls settle_share; the crank settles closed orders before
   each buy (`sf-settle`). New events BuybackShareHeld / BuybackShareRefunded; BuybackAccrued
   now fires at release. Config grows by bb_locked + bb_refunded (readers updated: x402
   `/buyback` held/refunded, `pearl buyback`, explorer decoder, both wallets). x402 overhead
   default 12M -> 14M (record rent). storefront.so `57067ffa...fc1f`. Tests: storefront
   LiteSVM 22 (held shares not crankable, settle on pending refused, finalized released,
   refunded/expired paid back + record rent, settle twice refused, crank leaves held shares,
   refund after a crank still paid in full), x402 11, relayer 33, extension 63, pearl-cli 36,
   explorer 20, light client 14 + 2, pearl-wallet 103 + checks (8a37729). agent-credit pins no
   storefront.so; it pins lightclient.so, re-pinned to the parallel-relay build `8d13918c...7882`
   (m55 + m56 80 steps PASS, pushed 3e7d991; Mac token/target/deploy/lightclient.so updated
   too). Demo on the VPS 11/11 PASS (216.5 s, 2026-10-18 17:21-17:25 UTC; restarted at 17:20
   after the header-relay agent finished, then kept up again from 17:30): step 5 checks the
   share is held and not crankable; step 7 (after finalize) settle_share releases it, buy 1
   half to the mining reserve, buy 2 the carry burned; step 9 the refunded order gets its
   1,112,043-lamport share back in the relayer's refund tx (record closed, bb_refunded up by
   exactly the share). Demo fixes: light client max_future_ms 1 h on localnet (header-relay
   set-params; production 120 s kept), and step 10 retries prove_header with a fresh proof
   when the faster relay grew the accumulator in between (AccProof).
14. HEADER-RELAY CATCH-UP (bug, owner "fix any bugs" 2026-10-18): DONE 2026-10-18 (commits
   e02eaf1..; SPEC 5.3 "Parallel relay", "Relay cost on real Solana", "Solana clock").
   Symptom: on the long-running VPS stack the relay verified ~12,600 of ~24,460 headers, 8-9
   headers/min against 40 blocks/min (1.5 s demo blocks), so no proof-mode bridge-out was
   released. Root cause 1 (protocol + relay): `submit_header` required a Verified parent, so
   headers were verified strictly one at a time, each a chain of ~12 confirmation rounds (37
   txs: submit, begin + chunk, 26 chunks as one batch, 9 steps, plus mark_final / close);
   at the box's load-30 slots (366 ms, not the nominal 50 ms) that was 7.5 s per header;
   idle it was 1.4 s, already zero margin against 1.5 s blocks. Root cause 2 (environment):
   the `--ticks-per-slot 8` validator's clock lags wall time ~30 s/min under load and the
   light client refuses headers > max_future_ms (120 s) ahead of it, so even a fast relay
   is capped at ~19 headers/min there. Fix: lightclient takes children of merely submitted
   parents and marks Verified only with a Verified parent (else WorkDone, `connect` later),
   same invariant and security (reasoning in SPEC); header-relay pipelines `--parallel 8`
   headers (src/relay.rs), batches polls/reads, fire-and-forget global ops, planned CU
   limits, `lag {...}` JSON line every 10 s, `status` lag_blocks, `set-params`,
   `close-works`, clock-wait notes. Tests: lightclient LiteSVM 14 (+3: out-of-order verify +
   connect, forged pending parent never Verified, close_work), header-relay catchup 3
   (sequential falls 15 behind in 96 slots at 1 block / 4 slots; parallel 8 drains a 30-block
   backlog in 67 slots, then lag <= 3; follows a node reorg below the LC tip).
   LIVE (VPS): stack restarted by this agent via `scripts/vps.sh demo` at 16:36 UTC
   (buyback agent DONE, stack was down; DEMO_KEEP_UP, stack left UP); demo 10/10 PASS
   (295.8 s) with the new relay. Then the
   relay hit the clock cap (lag 130); 16:48:42 UTC relay restarted (PID file updated) and
   `set-params --max-future-ms 86400000` on the VPS light client: lag 129 -> 1 in about 3
   minutes (50-70 headers/min); `pearl bridge out 1` at 16:51:39 -> withdrawal #3 released
   by release_proof 18 s later (final header 659). Sustained 16:51-17:18 UTC at load ~26
   (int7 experiment + stack): node 641 -> 1654 (38 blocks/min), 160 lag samples, max lag 9,
   mean 2.9, failed txs 0, clock waits 0. 17:18:28 UTC relay restarted once more (counter
   fix 2f.., same args, PID file updated); stack left UP, light-client max_future_ms 24 h.
   Cost on real Solana per fallback header: 37 signatures (0.000185 SOL) + priority on
   ~12.2M CU limit; ~0.27-0.44 SOL/day at 60 s blocks; 0.49 SOL refundable rent per work
   account. Follow-ups (not done): demo validator at default ticks per slot or init with a
   big max_future_ms on localnet (scripts/ owner); fallback blocks could skip the 24.7 KB
   proof upload (A, B derive from the header; ~70% of fees); demo block time 1.5 s is 40x
   the spec's 60 s.
15. CADRE API (public site backend, owner 2026-10-19): DONE 2026-10-19 (commits 64a4177..;
   docs/API.md, contract services/cadre-api/openapi.yaml). `services/cadre-api` (:10685, Node
   built-ins, no deps) serves /v1 over its own instance of the explorer Indexer + the
   explorer's page models: summary/feed/blocks/jobs/orders/payers, buyback, reserve, supply,
   revenue with wash screens, agents (REPUTATION 5.1 subset, sorted by screened P/SOL, never
   counts), mining/miners, accounts on both chains, bridge transfers + quotes, UNSIGNED txs
   (bridge deposit, x402 payment; BridgeLock as the window.pearl JSON form), relay of signed
   chain/Solana txs, inference models with exact storefront prices (storefront-x402 GET
   /quote now prices tickets), receipts, OpenAI pass-through /v1/openai/* to the gateway (SSE).
   CORS, token-bucket rate limit, ETag/304, one error shape, /v1/health + /v1/ready. Typed
   client services/cadre-api/client/index.ts. 22 tests (fake stack, web3.js byte checks,
   storefront verifyExact on the built x402 tx, OpenAPI route parity). LIVE on the test box:
   https://cadre-api.64-95-13-106.sslip.io (caddy, 443 only, systemd cadre-api, own clone
   ~/cadre-api); live check 28 pass / 1 pending, figures cross-checked against node + Solana
   RPC through the tunnel. 501 until other agents land: prepaid balance/top-up (gateway SPEC
   5.4, flag CADRE_PREPAID=gateway), pump-buy / crank / fund_mining tx builders, circulating
   supply (token-side explorer module), profiles from apps/explorer/src/agents.ts when it exists.
15. TOKEN-SIDE BUYBACK CAP + PERMISSIONLESS fund_mining + PUBLIC NUMBERS (owner decisions
   2026-10-18, SYNTHESIS items 6-7): DONE 2026-10-19 (commits 277e016..; SPEC 3.4, 5.1 "Reserve
   exit", "Reserve cap", "Public numbers"; docs/EXPLORER.md; TEST-REPORT last section).
   - storefront: with destination reserve, bought tokens go to the mining reserve while its ATA
     holds less than `bb_reserve_cap`, the excess is SPL-burned in the same crank
     (`BuybackToReserve` / `BuybackBurned`), `set_buyback_cap` (admin). Default cap 100M tokens
     (PROVISIONAL); 0 = no cap (what an upgraded old config reads).
   - reserve: `fund_mining` permissionless, rate limited (`set_fund_limits`: paused, 10M tokens
     per call, 3600 s, 20M per 24 h window, all PROVISIONAL), mining_pool / bridge_program frozen
     after the first funding, `migrate_config` grows old 276-byte configs. Creator-fee buybacks
     still land in the reserve uncapped (base design).
   - crank: `sf-set --reserveCap`, `fund-limits`, `migrate`, fund pre-checks the limits; `pearl
     buyback` shows the cap, next buy and per-buy split; demo step 2 funds with the trader key,
     buy 2 goes through the cap split.
   - public numbers: tools/defillama (DefiLlama layout; fees = OrderPlaced.sol_spent +
     BuybackShareHeld, holders revenue = BuybackAccrued; live check matched the Config counters
     on the VPS stack), explorer `/api/supply` (+ `/circulating`, `/total` bare numbers; cadre-api
     can import `supplyModel` from apps/explorer/src/supply.ts for its circulating_spl) and
     `/revenue` (self-dealing excluded, unique payers, concentration, seller screen).
   - Tests: reserve 18, storefront 23 (LiteSVM on the Mac with TOKEN_TARGET; litesvm's node
     binding aborts on the box), pearl-cli 36, explorer 28, defillama 2, pearl-wallet 134.
   - Devnet: upgrade steps in docs/DEVNET.md ("Steps for the 2026-10-19 token-side change"):
     deploy, then `crank migrate` on the reserve at once, then `sf-set --reserveCap`.
   - Demo with the new programs on the VPS: see TEST-REPORT (box stack was in use by other agents).
Not done in M2 (later): chunked blob manifests, persistent history index, multi-hop
blob push, getPeers across real hosts, USDC storefront, demo coverage of
PumpSwap/expiry/reorgs/restarts. agent-cli selftest's terms_hash vector keeps the old k on purpose
(mirrors the superseded agent/ vector; never submitted); its --pearl-job round trip uses k 2048.
Owner decisions still open: dev buy size, chain params (block time, K, alpha,
emission), name and ticker, storefront buyback min buy / interval / cranker (item 13), reserve cap 100M and fund limits (item 15, PROVISIONAL). If a session dies, check git log per path and
CLAUDE.md "Who is working on what".

16. AGENT LAYER (owner decisions 2026-10-18, SYNTHESIS 3/4/9/10 + RejectResult): DONE 2026-10-19
   (commits 5efc421, c904965..072290f; SPEC 2, 4.1, 4.2, 5.4; docs/EVALUATORS.md, RPC, CLI,
   EXPLORER).
   - RejectResult (tx kind 13): job evaluators reject the current delivery with threshold raw
     ed25519 sigs over `pearl-sol:reject:v1 || job_id || deliverer || result_hash`; job back
     to Open, Reject / RejectCount / RejectStats records (tags 14-16), the rejected deliverer
     cannot redeliver that plain job (closes first-deliverer griefing). RPC getJob
     reject_count, getJobRejects, getRejectStats, getAccountTxs role `rejected`. Vectors
     chain/agent-types/vectors/reject.json; codecs + byte tests in extension/ and
     ~/pearl-wallet (dev b3716f8e1, pushed). Light client / relayer unaffected (they prove
     CompleteJob / ExpireJob only; a rejected job completes or expires later as before).
   - Open evaluation: docs/EVALUATORS.md (what is signed, the plain-job check, reject only on
     provable deliverer faults, ticket rule, how to run); `pearl-evaluator`
     (services/attester, :10667): polls Delivered jobs naming its key, CompleteJob on accept,
     RejectResult on provable failure, holds otherwise, threshold > 1 via peers'
     GET /signatures/<job>. Tickets are accepted or held, never auto-rejected (glue not
     checked yet). storefront-x402 GET /evaluators; buyers already name evaluators with
     spec.evaluators / `pearl job post --evaluator`. services/agent-cli is the superseded
     SVM CLI and was not changed.
   - Agent profiles (no new state): apps/explorer /agent/<addr>, /api/agent/<addr>, /agents
     (ranked by third-party-signed P behind completed work, self-dealing excluded, stale
     hidden); `pearl agent <addr> [--json]` reads the explorer. Explorer index state version 2
     (re-indexes once). A2A agent card probe at RegisterAgent.uri (http(s), no private
     addresses, 5 s, 64 KiB), display only.
   - services/cadre-proxy (:10695): local OpenAI-compatible proxy with its own wallet (0600
     file, never printed), SPEC 5.4 wallet-sig / API-key auth, spend caps + ledger, SSE
     pass-through, loopback-only x402 top-up, config snippets for Hermes / OpenClaw / Kilo /
     Cline (README lists which formats are verified). NOT yet live-checked: no gateway was
     listening on the test box's 10664 at the time.
   - Not done: evaluator fee / bonds (owner: later), card JWS verification, persisted probe
     state, ticket rejection after the replay check, buyer-named evaluators through the
     gateway API (gateway agent's scope).

17. CADRE INFERENCE: STANDING JOBS + PREPAID GATEWAY (SYNTHESIS item 3, owner decisions 2026-10-18:
   prepaid balance, OpenAI-compatible gateway, receipts, measured throughput). Demo step 11 PASSES
   on the test box 2026-10-19 (11/11 in 740.5 s, log ~/demo-takeover2.log on the box): RegisterModel
   Llama-3.2-1B int7 (144 matrices, k shards <= 2048, 835 blobs, 1.78 GB, upload 55 s); x402 top-up
   of 256 rows (95411375 lamports); wallet-signed chat completion and streamed completion with
   receipts (ticket, order, rows, charged lamports, status + explorer links); 2 rounds, 288 standing
   jobs, 288 blocks; ticket completed (56 of 256 rows used, W pro rata), Order finalized by
   finalize_order_proof; settled 471 s after payment.
   - Root causes fixed in the takeover: the token validator kept only 10000 shreds (about 6 s of
     50 ms slots), so the demo could not find the finalize tx (scripts/localnet.sh now keeps 5M,
     LOCALNET_LEDGER_SHREDS); cadre-api's pass-through used fetch with a whole-reply 300 s
     AbortSignal (and undici's own 300 s headers/body timeouts), which cut a slow completion: it now
     uses node:http with a 30 min reply-headers limit and a 10 min stream-idle limit; the worker's
     torch pool oversubscribed under the miner (demo now runs it at cores / 2). The putBlob 413 was
     the node's default blob quota under a 1.78 GB model (fixed earlier by DEVNET_BLOBS in the demo).
   - Measured throughput (real runs only; data in docs/measurements/cadre-worker-2026-10-19.jsonl):
     4-core test box, torch 2.5.1 CPU. bench.py at load 7: 3.1 tok/s decode (8-token prompt), 2.9
     (64), 1.0 (256). Under the demo miner (load about 20): 0.41 tok/s at 4 threads, 1.33 at 2,
     1.04 at 1. Through the gateway in the passing demo (2 threads, miner mining the round's
     segments): 0.37 tok/s (chat, ttft 10.8 s) and 0.63 tok/s (stream, ttft 3.5 s). A GPU worker
     is the obvious next step; nothing here is a GPU figure.
   - MODEL: Llama-3.2-1B-Instruct (owner choice 2026-10-19), same recipe, k shards <= 2048, 144
     matrices, ~/cadre-models/llama-3.2-1b-instruct-w7a7-k2048 on the box (quantize.py --model
     <unsloth/Llama-3.2-1B-Instruct snapshot> --name llama-3.2-1b-instruct-w7a7); the demo
     registers it by default (/v1/models id llama-3.2-1b-instruct-w7a7, the only served model; the
     base quantization stays on disk). Chat uses the tokenizer's chat template and stops on
     generation_config's eos (<|eot_id|>). Demo 2026-10-19 23:40 UTC run 11/11 (740.6 s): signed
     chat "What is the capital of France? Answer in one word." -> "Paris" (2 tokens, receipt order 4).
     Through cadre-proxy: "Name three primary colors, comma separated." -> "Red, Blue, Yellow."
   - Cadre web asks (CLAUDE.md) all served in cadre-api (8b31544, 0f912c2): CORS allows
     X-Cadre-Wallet + DELETE and exposes the receipt headers; prepaid proxied by default
     (/v1/inference/balance, balance/topup(+quote), balance/close, prepaid/{account}); pass-through
     serves keys, balance, prepaid, receipts, stats; served_by_gateway compares cadre.model_id (live:
     true); unsigned pump-buy (pump buy_exact_quote_in_v3 on the curve, PumpSwap after), crank-buyback,
     fund-mining (simulated against the box's real pump/storefront/reserve programs); credit line from
     the payout lien in /v1/accounts; card_probe on agents; registered agents listed before paid
     work; crank txs accept `{payer}` (the site's field). Test box cadre-api runs main with
     CADRE_PREPAID=gateway. ~/cadre-web sends X-Cadre-Wallet (2ddf965), e2e checks tightened (33ccde4).
   - Worker memory: a decode step's [:r] view kept the 32-row padded int32 product alive (generate
     keeps every step's C), about 50 MB per token; the box worker was OOM-killed at 7 GB during the
     e2e. Fixed bdf5a80 (64 tokens: peak RSS 6.4 GB -> 2.3 GB, measured).
   - cadre-web e2e against the box: 18/18 clean (2026-10-20 01:21 UTC, strict x402 check, Instruct).
     The earlier wallet-tx "60 s timeouts" were the e2e itself: pump() answered approvals until 3 s
     passed without one, counted from the click, but the site first builds the tx through cadre-api
     (seconds over the tunnel), so a later approval stayed unanswered, the step's 60 s result wait ran
     out and the next step answered the stale window. pump now waits for the approval an action must
     raise (~/cadre-web a9c903a).
   - cadre-proxy live check (2026-10-20): wallet funded on the box localnet, `topup --rows 128 --pay`
     (201, order 5, 60261244 lamports), a chat request through :10695 answered with the gateway's
     receipt headers, the ledger booked 23068759 lamports / 49 rows, a request over the day cap got
     429 cadre_spend_cap. Fixed on the way: the proxy refused the real storefront's requirements
     (fee payer = payTo).
   - Open: a GPU worker (CPU decode is 0.4-1.6 tok/s).

18. PUBLIC DEVNET (owner decisions 2026-10-18; finished 2026-10-19 by the "devnet finish" agent):
   LIVE, details and ids in docs/DEVNET.md. Agent chain on 3 BitLaunch boxes (rolling upgrade to
   fe8e04a: RejectResult kind 13, RegisterModel / PostSegments, token-side changes), Solana devnet
   programs deployed + initialized from fe8e04a (reserve 8m1RBPii, bridge 3S226LmV, storefront 9XjvGk2x,
   lightclient 47mavKip; upgrade authority the deployer), CADRE mint BwJqbsJUXshRj9ABxXs4RZ838kZrSqw8TT4CKMNfEu5e
   (real pump.fun create_v2; devnet's pump Global has 0.3 SOL virtual reserves, so the dev buy graduated
   it and it trades on PumpSwap; storefront buybacks burn since the reserve is above the 100M cap).
   cadre-api https://api.64-95-12-116.sslip.io. Wallet release cadre-devnet-6.73.4 (pearl-wallet).
   Fixes on the way: crank trade.ts PumpSwap path; pearl-miner --deliver orphan sweep (jobs mined by
   non-delivering miners); header-relay confirm-poll / send-gap / fresh blockhash / --no-relay and the
   relay moved to box 1 (Solana devnet public RPC limits, DEVNET.md "Solana devnet RPC budget"); explorer
   SOL_POLL_MS. e2e (DEVNET.md "End to end results"): run 1 FAIL at step 4 (light client behind; fixed),
   run 2 7/7 PASS (130.9 min). Owner 2026-10-20: dedicated Solana RPC from keys/devnet/solana-rpc-url;
   plumbing done (deploy/with-rpc.sh, devnet-deploy.sh rpc-url, URL redacted in logs), file NOT provided
   yet, services on the public RPC. NEXT: when the file exists, `scripts/devnet-deploy.sh rpc-url`, ctl
   restart the Solana units on boxes 0-2, then scripts/devnet-e2e.sh twice. Header relay above 10 slots
   stops confirming from one IP (public RPC; recheck on the dedicated one).
19. DEVNET SITE + PAID AI + MAINNET PREP (owner decisions 2026-10-20 a-d; "devnet site + AI" agent):
   - Website LIVE https://cadre.64-95-12-116.sslip.io (cadre-web.service on box 0 from cadre-web a9c903a,
     `scripts/devnet-deploy.sh site`), installer at /install-miner.sh with devnet defaults (dry run walks
     all 8 steps). Headless Chromium: 8 pages + job/block/agent detail, desktop and phone, no console
     errors, live data. GAP: the installer clones the private pearl-sol repo, so a public install fails
     at the source fetch (owner: publish source, a miner tarball or binaries).
   - Paid AI LIVE on box 0: Llama-3.2-1B-Instruct int7 registered at height 2293 (model_id df34fa5f...),
     cadre-worker (2 threads, 2.0-2.1 tok/s measured under the miner) + cadre-gateway units, cadre-api
     gateway + prepaid on. Real paid flow (scripts/devnet-inference.sh, run devnet-ai-1): x402 top-up
     of 256 rows (95,411,375 lamports), wallet-signed chat "What is the capital of France? Answer in one
     word." -> "Paris." with receipt, Order 12 finalized by finalize_order_proof 27.3 min after payment
     (tx 2ap8tsmZ...), all 144 segments mined in 138 min. Details: DEVNET.md "Inference".
   - OPEN: nodes 1-2 never fetch the model manifest blob (p2p fetch of a push=false pinned blob), so
     only box 0's miner mines segments; BOX1/2_BLOBS (8 GB) are in devnet.env but not installed
     (needs a node restart on boxes 1-2). Gateway receipts' status_url points at 127.0.0.1:10664 (the
     gateway's --public-url cannot express cadre-api's /v1/openai prefix; gateway/cadre-api change).
     cadre-web AI page: the models table's Context column is clipped on desktop (inner overflow).
   - Wallet: Cadre Devnet preset siteUrl set (pearl-wallet dev + danishseal cadre-chain 6e41f7ee2); not in
     the released zip cadre-devnet-6.73.4.
   - pearl CLI waits 600 s for inclusion (was 60 s, too short at 60 s blocks).
   - docs/MAINNET.md: runbook, keys, config values, pump curve (mainnet 30 SOL virtual vs devnet 0.3;
     scripts/mainnet-prep/curve.cjs reads both Globals), measured costs, checklist, owner decisions.
     No mainnet transaction was sent.

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

Steps and last run (2026-10-17, all 10 PASS, 197.2 s without builds; 132.8 s before the
lien step, 122.8 s before the refund step, 74.6 s before the light client). Step timings
below are from the 8-step run; the 10-step run: 13.1 / 11.4 / 19.5 / 6.1 / 12.7 / 22.3 /
29.6 / 23.5 / 53.5 / 5.5 s.
The relayer runs with REFUND_GRACE_MS 5000 (DEMO_REFUND_GRACE_MS).
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
8. (20.4 s) refund: AdminConfig raises min_protocol_fee to 1e16 (above any P), a new x402
   order is held back (protocol_fee_below_min), refunded after the grace period by
   `refund_deposit` (asserted from the refund record's tx) + `mark_refunded`: the Order's
   buyer (storefront wallet) gets exactly Order.tokens back, Order status Refunded;
   floor restored, the deposit is still not unlocked; invariant exact, nothing in flight.
9. (53.5 s) payout lien (R2): the miner payout key sends SetRoute (50% to a demo lender
   key, release key a demo key); a user tx with a 2-token fee gives it income; its
   matured coinbase is split by the chain into a withdrawal entry from the miner to the
   lender (routed 1,000,000,500, withdrawn 1e9 units, 500 carried), released on Solana by
   `release_proof` (header-relay --settle; asserted from the Processed PDA's tx), 1,000,000
   SPL to the lender; ClearRoute by the release key ends it. R1: the SetRoute block's
   receipt is checked against getHeader's tx_root by its path. R5: accumulator holds 75
   final headers; `acc-proof` + an on-chain `prove_header` for an early header.
10. (5.5 s) teardown by PID incl. header-relay, ports 10600-10662 free.
Final invariant: vault 18,836,393,149,842 SPL x 1000 = 1e18 - escrow, nothing in flight,
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

Not covered by the demo: PumpSwap (graduated) path, the expire path (ExpireJob + W back),
USDC, reorgs
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
