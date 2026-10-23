# pearl-sol handoff

> 2026-10-20: session handoff (account switch) in docs/SESSION-HANDOFF.md (history now; the current state is the next section).

READ THIS FIRST in a new session. Update it after every step (decision, commit,
agent result). Keep it current, not historical: rewrite stale lines instead of
appending.

## Current state, 2026-10-23

Checked 2026-10-23 ~03:40 UTC against the live devnet, read only: cadre-api (`/v1/ready`,
`/v1/summary`, `/v1/hosted`, `/v1/strategies`, `/v1/swarms`, `/v1/connect`), `getChainInfo` and
`getParams` on all three nodes, and `systemctl` on the boxes. Docs index: docs/README.md. Items
1-47 below are history; an item a later decision replaced carries a SUPERSEDED line.

**Facts that hold everywhere now**

- The town and DMs are on Solana, program `DAyyoBkSmyf3guaYpPp8FCAjRUPB5jZ9DCw6pMZCzGnZ`
  (TOWN-ON-SOLANA.md). Tips and paid DMs are SOL in lamports, not CADRE. All three nodes refuse
  town and DM kinds from height 12000 (`town_sunset_height`, provisional; at 30 s blocks that is
  about 42 h after tip 6998). Old history stays readable; ReclaimMessage (17) stays valid.
- User agents trade PRACTICE only: pretend money at real Solana mainnet prices, committed on the
  strategies contract (STRATEGIES.md, SPEC 4.8). Real swaps sit behind `trade.real_swaps`, which
  no creation flow sets. House traders are separate (TRADING.md).
- Creating an agent is one screen with two wallet prompts: a signMessage over the spec (it also
  authorizes hosting) and one Solana tx with the SOL. The step-by-step form is only
  `/agents/new?advanced=1`.
- Hosted agents buy 8,192-row inference tickets, the largest the chain allows (`max_balance_rows`
  8192; item 47).
- Chain: 30 s target, finality 60, uncles, all from height 6000 (SPEED.md).

**What runs where**

| Box | Runs |
|---|---|
| 0, lode-devnet-0, 64.95.12.116 (checkout main 8821e75) | cadre-node, cadre-miner, cadre-attester, cadre-relayer, cadre-settle, cadre-header-relay, cadre-storefront, cadre-crank, cadre-explorer, cadre-api, cadre-web, cadre-gateway, cadre-worker, cadre-agents (hosted runtime, 12 agents), cadre-swarm, cadre-desktops (30 agents, 36 desktops), cadre-practice, cadre-agent-dump, caddy, cadre-health.timer. Not running: cadre-trading, cadre-feed-bot |
| 1, lode-devnet-1, 64.95.13.213 | cadre-node, cadre-miner, cadre-header-relay, caddy |
| 2, lode-devnet-2, 64.95.12.22 | cadre-node, cadre-miner, cadre-header-relay, caddy |
| test box, 64.95.13.106 | cadre-api (test-localnet, idle), caddy, x-agents (not Cadre); by PID (run-*.sh, *.pid): house traders `~/trading` and market desk + judge `~/market-build` on main c286391 with `TOWN_PROGRAM` + `SOLANA_RPC` (Solana town, 2026-10-23); self-hosted agent `~/usertrade` (agent1.pid) still on its 2026-10-21 code with no TownNode, so its posts stop at 12000 (open issue 7) |

Solana devnet programs (DEVNET.md): reserve `8m1RBPiiscHWpXRDj4NevSFMN68tbgCC8f4hu6N4hVzi`, bridge
`3S226LmVvsoA4QrEpwytiuLxREJrQHS7A1UkeDsJGFoN`, storefront `9XjvGk2xJdByPL6NmUCQg7RdBLyeDjWjikZBnwuWvAKu`,
lightclient `47mavKipT2URcdj9qp2Stv6H5cM41cmzDXiTEcmDie4m`, town `DAyyoBkSmyf3guaYpPp8FCAjRUPB5jZ9DCw6pMZCzGnZ`,
mandate `BG5WkxZyYwWXTY2YSAJGhwL7JcxChRXx8UqHEAMAC5yc`. CADRE mint `BwJqbsJUXshRj9ABxXs4RZ838kZrSqw8TT4CKMNfEu5e`.

Cadre contracts (deploy/*.json): market `E1kpbaHn14z4Amdqb7SGjLbn576q1KPC3haAG6YQdsym`, strategies
`BvYwLdhADUUHcxUufQig4ZufpoJoN1EZmtTHxf6UZSXr` (from 6160), swarms `654yr1HroSyqBosqFmn2n62MAzjfyrPVMzhjLTDNMQmt`
(from 6352), toolbelt `5EVZHQRgQkLTthSXzKJck9AFpdNcuYMzi2nde1bNokpB`, halt
`88uaR3ea34nB1KGAPGZpvRYyUvLvKjLjopWyM54dpAPL`, Cadre AMM pool `65QGmPmEyGBVFic2A5sDvnKWW9Kw4yWCHLA9sWRe4ZFn`,
mandate contract `JCs8ZiUdUq3myCHK7M9r8LCepRrkAKwyY6EN8u1w3VT7`, agent token
`GqHzh8yz4jNkHJhdgEV8ooJ8WVfzCxTVGh4cTFK7GWdw` (trading-devnet.json).

**Known open issues**

1. (Fixed 2026-10-23, town follow-through) Solana tips and DM pay rank on `most_tipped_sol` and
   `paid_dms_sol` (lamports); `most_tipped` and `paid_dms` stay the CADRE Cadre-chain history.
   Live after a box 0 cadre-api restart.
2. Toolbelt: merged, and the box 0 runtime serves its status route, but none of the 12 hosted
   agents has a toolbelt. The TOOLBELT.md 11.1 e2e delivery is unfinished.
3. The paid planner for one-screen create is off (no `AGENTS_PLANNER_KEY`); the site matches keywords.
4. Gateway request data: 8.59 GB waiting at the 8 GB pressure line; the 64 GB cap is item 37's
   stopgap, to revert after the drain. Storefront: 64 of 93 orders pending (`/v1/summary`): all paid and on the chain; 9 wait on
   the light client, stuck at 6300 since 2026-10-22 21:39 UTC until the header-relay uncle fix is deployed;
   7 payments (0.1769 SOL) were taken without an Order and need the refunds in DEVNET.md (item 51).
5. Desktops: one unhealthy (campion66 Social, "browser host down"); model 402 pauses are the
   desktops resilience row's work.
6. Connect SDK and py: local posts and DMs still need the Solana town builders (item 43).
7. The self-hosted agent on the test box (`~/usertrade`, tide_ledger, item 33) was set up on
   2026-10-21 as a real-swap trader, before the practice-only rule. Stop it or move it to practice.
   Its code predates the town move; main would run its trade section as practice (no `trade.real_swaps` in
   its signed spec), so it was not updated: town posts from it fail from height 12000 until the owner decides.
8. Strict auth (`CADRE_AUTH_REQUIRE_AUDIENCE=1`) waits for a wallet release (item 28).
9. Swarm sponsor float is off until the owner sets its key (item 45).
10. The test box demo stack (scripts/demo.sh) is down since the 2026-10-21 reboot.
11. (Fixed 2026-10-23, practice-polish, NOT DEPLOYED) Swarm kickoff resumable: swarms.json keeps the
    kickoff state, the plan and the one planned hire; a stop no longer marks it failed, `run()`
    resumes it, a lead whose inference top-up is held waits (`waiting_budget`) and the tick resumes
    it when the hold lifts; the planned hire is sent at most once (a hire from the lead to that
    member already on the market counts). Devnet swarm 1 ("QA rql2", lead 8B5Tv9...) shows "the
    runtime stopped before the kickoff"; after a box 0 cadre-agents restart on this code it resumes
    by itself. Its lead's inference is held by its daily SOL cap until 00:00 UTC and its wallet held
    0.0165 SOL against a 0.0251 SOL quote for its usual ticket on 2026-10-23, so the kickoff may
    then wait on a wallet hold until someone sends the lead SOL.
12. (Fixed 2026-10-23, qa2, NOT DEPLOYED) Light client stalled at 7497 since 07:50 UTC on an equal-work fork:
    lightclient program upgrade, then header-relay on boxes 0-2 (item 56).
13. (Fixed 2026-10-23, qa2, NOT DEPLOYED) Hosted agents' profile blobs are pruned by the node (their SetProfile is on
    Solana): the runtime puts them back; a lasting pin is open (item 56).

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
   - Open: a GPU worker (CPU decode is 0.4-1.6 tok/s). Measured 2026-10-20 on an A40: item 20.

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
   Light client (2026-10-20, "light-client VerifyFailed" row, resumed): relay re-send fix de8edd5 was
   already on boxes 1-2, but cadre-settle on box 0 ran an older build; all three now run header-relay
   098fbb52... from main 1a1f4f8. Two more relay fixes found in the logs: the lanes raced on shared
   steps (submit Custom(0), accumulate 6023: 42 failed txs in 55 min) and both registered every job;
   now the lane that owns the height sends a shared step at once and the other only after 45 s
   (295a4cb, 1a1f4f8), and the jackpot names the header's submitter as reward recipient (4665990,
   WrongRelayer 6021 with lanes). Test two_lanes_send_each_shared_step_once (6/6 catchup PASS on box 0).
   Watch 06:38-08:40 UTC: zero 6015/6008/6016/6023/Custom(0), one 6027 per lane, lag 0 at the end.
   Rent: 14 of 20 relay work accounts closed (5.1070 SOL back to the relay payers), 6 in use (2.1887 SOL).
   e2e runs 3-9: 3 and 6 PASS 7/7, 4/5/7/8/9 FAIL on Solana public RPC 429s from the Mac's IP (shared
   with a ~/levpad keeper on the Mac); retries added (64834ca, deb4db9, deb99e3). Two consecutive passes
   NOT reached; needs the dedicated RPC (also for the Mac scripts) or an e2e host with its own IP.
   Boxes: kernel 6.8.0-147 + libc6 installed, reboot pending on all three (not done here).
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
   - FIXED 2026-10-20 ("Cadre leftovers" agent): nodes 1-2 never fetched the model manifest because no node
     had it (the CLI registered, hit its 60 s inclusion wait and stopped before uploading). CLI uploads first
     now (e9e28a8), the blobs were uploaded to node 0, nodes 1-2 got the manifest with no restart and run 8 GB
     blob quotas; pearl-node asks for all of a file's chunks at once (68a43c8); all three miners mine
     segments. Receipt links: cadre-gateway --public-api-root (5488d42), devnet and test box receipts point
     at <api>/v1/openai/requests/<id>. cadre-web AI page models table fits its card (cadre-web 651e702,
     redeployed). Boxes rebooted into kernel 6.8.0-147. Details: DEVNET.md "Inference", TEST-REPORT.
   - Wallet: Cadre Devnet preset siteUrl set (pearl-wallet dev + danishseal cadre-chain 6e41f7ee2); not in
     the released zip cadre-devnet-6.73.4.
   - pearl CLI waits 600 s for inclusion (was 60 s, too short at 60 s blocks).
   - docs/MAINNET.md: runbook, keys, config values, pump curve (mainnet 30 SOL virtual vs devnet 0.3;
     scripts/mainnet-prep/curve.cjs reads both Globals), measured costs, checklist, owner decisions.
     No mainnet transaction was sent.

20. GPU INFERENCE ON RUNPOD + 8B INT7 CHECK (2026-10-20, "RunPod GPU inference + 8B int7 check" row):
   - tools/cadre/worker.py --device cuda (also CADRE_DEVICE): weights, scales and glue on the GPU, the int32
     products through torch._int_mm on CUDA, TF32 off; CPU path byte-identical to before (A/C files of all
     144 matrices identical old vs new on the test box; test_cadre 5/5). tools/cadre/gpu_check.py: GPU C vs
     CPU int_mm of the same A and numpy int64, 0 mismatches in 151,451,392 elements, so GPU segments verify
     everywhere; A itself differs between devices (fp32 glue), GPU runs repeat exactly.
   - Measured on one A40 ($0.59/hr): 1B Instruct 24.5-25.1 tok/s per worker process (ttft 0.05-0.12 s),
     84.9 tok/s summed over 8 processes; 8B Instruct 8.0-8.6 tok/s. Paid chat through the test box gateway
     with the GPU worker: 21.13 tok/s, 8.7 s wall for 32 tokens (CPU worker same chat: 0.376 tok/s, 110.6 s).
     $/1M tokens and details: MAINNET.md section 7, TEST-REPORT "GPU worker on RunPod".
   - 8B accuracy (research/INT7-ACCURACY.md section 7): Llama-3.1-8B-Instruct WikiText-2 PPL 8.3131 fp16 ->
     8.5664 int7 (+3.05%); ARC-E/ARC-C/HellaSwag within 2 points of fp16 on 200 items each. 1B Instruct +2.51%.
     The 8B has 640 matrices, over RegisterModel's 384: the chain change is item 21.
   - scripts/gpu/runpod-{create,bootstrap,stop}.sh (one pod, refuses a second; throwaway ssh key in .gpu/,
     gitignored; the pod clones the public repo and quantizes from public HF weights). tools/cadre/remote_worker.py
     puts a remote worker behind a gateway (forwards HTTP, copies each request's segment files back).
   - Left: nothing running (pod terminated, test box back on its CPU worker, pid in .demo/pids). Open: a
     standing GPU worker (provider, card count, worker next to the gateway so segments are not copied), the
     384-matrix limit for 8B (DONE, item 21), the worker's per-token Python overhead (decode does not depend on context here,
     so it is launch-bound, not GPU-bound).

21. REGISTERMODEL MATRIX LIMIT 384 -> 1024 (owner-approved 2026-10-20 for the 8B int7 model; "RegisterModel
   matrix limit for 8B" row):
   - Rule (SPEC 4.5): `MAX_MODEL_MATRICES = 1024` (chain/agent-types tx.rs, pearl-job standing.rs). A
     RegisterModel tx may be up to `MAX_REGISTER_MODEL_TX_SIZE = 48 KiB`; every other kind keeps 16 KiB.
     Coordinated upgrade height per chain (`max_model_matrices(chain_id, height)`): public devnet (chain
     1128350802) keeps 384 below height 4200; every other chain has 1024 from genesis. Relaxation only:
     existing blocks stay valid; below 4200 old and new nodes refuse the same blocks. Every devnet node
     (including any outside node built from the NODE.md recipe) must run this commit or later before 4200.
     The node RPC refuses an over-limit RegisterModel at admission.
   - Where the limit lived: agent-types codec + executor + SPEC only. Not bounded elsewhere: the light
     client never parses RegisterModel (PostSegments repeat k, n, hash_b), the gateway, worker, explorer,
     cadre-api, extension and ~/pearl-wallet read the matrix count with no cap, p2p messages are 8 MiB,
     blocks 16 MiB, RPC bodies 2 MiB, state values 1 MiB.
   - Measured (test box, release, executor alone): RegisterModel tx 25,808 bytes at 640 matrices (41,168 at
     1024; the largest name and URI make it 26,314 / 41,674); the Model record in state 25,849 / 41,209
     bytes; executing the registration block 0.19-0.23 ms (640) and 0.24-0.30 ms (1024) against 0.11-0.14 ms
     (144); one PostSegments tx of 64 segments 0.37-0.73 ms whatever the model size (the box ran at load
     about 18), so a full 640-matrix round (10 txs) is 4.1-7.0 ms of block execution.
   - Tests: agent-types 2 (codec limit and sizes incl. a 20 KiB non-RegisterModel refused; schedule),
     executor standing 3 (640-matrix Llama-3.1-8B-shaped model registers, a full round of 640 segments in
     10 PostSegments posts and its last matrix mines, 1025 refused; devnet refuses 640 at 4199 and takes it
     at 4200, 384 still fine before; the cost measurement), agent-node network 1 (640-matrix tx sent to
     node 1, gossiped to node 0, mined, the block reaches node 2, getModel 640 on all three; 1025 refused
     at the RPC). Chain workspace 102 tests green on the test box.
   - End to end (test box, own dir ~/regmodel, ports 10650-10657, 2-node private chain, real agent-node +
     `pearl model register` on a synthetic 640-matrix manifest, k 2048 x n 8 each, 1280 files): (a) chain id 1347616811: 640
     registered (tx included, getModel 640 on both nodes, `pearl model fetch` on node 1 rebuilt all 1280
     files from blobs and checked every hash_b), 1025 refused at sendTx ("too large: matrices"), 1024
     registered (getModel 1024 on both); (b) devnet chain id 1128350802 on a fresh private chain: 640
     refused at sendTx at height 1377 ("1..=384 matrices"), 384 registered at 2255 and fetched; (c) the same
     chain id, crossing the height: 640 refused at 1440, then after height 4215 registered at 4427, getModel
     640 on both nodes, fetch on node 1 OK, both tips equal (5580). Script: e2e.sh in the session scratchpad
     (not committed). Found on the way: `pearl model register` uploads every blob before the tx, and an
     unreferenced blob lives `grace_blocks` (default 100) blocks; the first run's fast private chain
     pruned the manifest before the tx landed. At the default put rate (1 MiB/s) the 8B's 7.5 GB takes
     about 2 h, longer than 100 devnet blocks: the node an 8B is uploaded to needs a larger `grace_blocks`
     (or put rate) for the upload.
   - crates/pearl-cli/Cargo.lock refreshed (ureq's tls feature had added rustls, ring, webpki-roots and
     friends without a lock update); `cargo build --release --locked` passes on the test box.
   - Devnet rollout DONE 2026-10-21 (combined with the Phase 1 town merge, main 816c941: matrix height
     4200 + social height 3260): the tip (4148 at 13:30 UTC) was already past 3260 on old binaries, so first a
     replay check: the 4162 finalized devnet blocks (raw, from node 1's RPC) through consensus_pow::Chain with
     the Pearl verifier and the new code reached the identical block hash at 4162 (a6f552a4...), 17 s on the
     test box (scratch tool, not committed). Then agent-node 7ce3d9a1... + pearl-miner fe576acc... built on box 0,
     cadre-node + cadre-miner restarted on box 1 (14:22 UTC), box 2 (15:10), box 0 (15:11); after each, the
     three nodes had the same tip, finalized height and finalized hash, 3 peers, not syncing. Height 4200
     passed between box 1 and box 2 with no RegisterModel over 384 sent. After all three: finality moved
     4219 -> 4223 with equal hashes, box 1's miner 20 blocks accepted 0 rejected since its restart, box 2 mined
     tip 4253, every cadre-* unit active on all boxes. Old binaries kept in ~lode/{agent-node,pearl-miner}.prev.
     Seen on the way: a fresh node on the test box (old and new binary alike) stalled at height 1-2 syncing
     devnet over p2p ("orphan pool full" on gossiped blocks); the NODE.md join recipe synced on 2026-10-20, so
     the header sync path needs a look (not this row's change: the old deployed binary does the same).
     Closed by item 24 (the orphan pool cap, not a commit; fixed in the sync-fix commit).
   - Not done (owner, 2026-10-20): no 8B registration on devnet, no GPU; devnet stays on the 1B and the
     CPU worker. Open for an 8B: the ticket budget and the per-segment register_job cost are DONE in item 27,
     and quantize.py's equal-shard rule for vocabularies like Qwen3-8B's (MAINNET.md section 11 item 11).

22. ECOSYSTEM PHASE 1, "THE TOWN IS ALIVE" (M1.1-M1.5, 2026-10-20/21, "ecosystem Phase 1" row):
   SUPERSEDED 2026-10-22 (item 42): the town moved to Solana; tips are SOL. The Cadre-chain kinds below stop at height 12000.
   - Owner decisions recorded in docs/ECOSYSTEM.md 5.1: D2 outside AI price = 2 x the measured A40 cost
     ($3.86 per 1M tokens) = chain config key 25 `price_mlamports_per_gflop` 14736 (derivation there, SOL/USD
     105.985 Coinbase spot 2026-10-20 20:00 UTC); D1/D3 about 30 house personas on the 1B CPU worker, A40 +
     8B + hundreds of agents later with a daily cap (no GPU started); D4-D9 defaults accepted.
   - M1.1 chain (SPEC 4.6): SetProfile 14, OperatorLink 15, Post 22, React 23, Follow 24 (agent-types
     social.rs, executor social.rs, state social.rs, rpc social.rs), social fee rule `max(min_fee,
     social_min_fee + social_byte_fee x len)`, handles first come, house flag only for a declared house
     operator with its OperatorLink, post refs, tips, follows; config keys 19-26 (encoded only once set, so
     the config record matched old nodes); profile blobs pinned while referenced; devnet activation height
     3260; vectors chain/agent-types/vectors/social.json. RPC getProfile, getHandle, listProfiles, getPost,
     getFollowCounts, isFollowing.
   - M1.2 apps/explorer social.ts (town index, thought receipts, text avatars, house spend line) and cadre-api
     town routes (docs/API.md "Town"). M1.3 ~/cadre-web /town, full profiles, /u/[handle], /leaderboards
     (live: hardest thinkers, most tipped; the rest are labelled stubs with reasons); ~/pearl-wallet dev
     660ced61f social codecs, approvals in words, profile editor, Town page and profile (also on danishseal
     cadre-chain). M1.4 services/swarm (house swarm v0, README). M1.5 `max_balance_rows` (balance-form
     tickets above 4096 rows, pro-rata pay unchanged), cadre-gateway per-ticket `round_secs` (large balances
     every 3 h by default) + round cap 2,044 rows (one 4 MiB A blob at k 2048) + `text_sha256` receipts,
     storefront-x402 reads the price knobs from the chain.
   - LIVE on devnet since 2026-10-21 (docs/DEVNET.md "Town"): 30 house profiles, verified posts on
     https://cadre.64-95-12-116.sslip.io/town (thought badge = sha256(body) equals the receipt's text_sha256).
   - Open: the site's Privy modal (teammate redesign 99a6d08) logs console errors on every page of the devnet
     host (auth.privy.io frame-ancestors and a 403 on analytics): the Privy app's allowed origins need
     cadre.64-95-12-116.sslip.io (Privy dashboard, owner or teammate). The first 8192-row ticket's first
     settlement round is due 3 h after its first request (round period), so `round_posted` for it lands
     after this report. The wallet's town screens were checked headless on mocks, not signed live on devnet.
     Phase 2 (DMs) next.
23. USER AGENTS: "SPAWN YOUR OWN CADRE AGENT" (owner 2026-10-21; ECOSYSTEM 7, SPEC 4.8, API.md "User agents"):
   SUPERSEDED in part: the multi-step wizard is now `/agents/new?advanced=1`; the default is one screen with two
   wallet prompts (spec signMessage, then the SOL tx); spec v1 creator controls were dropped for v2 sovereign agents (item 33).
   BUILT. services/swarm/agents: AgentRunner subclasses the house swarm's Swarm (operator = the user;
   same tick/budget/paid thoughts/filter/encoder), agent spec v1 signed by the operator's Solana
   wallet and committed in the profile blob, per-day SOL/CADRE/row caps from the agent's own wallet,
   receipt posts (`receipt sol:<sig> ...`) anchoring Solana actions on Cadre, the agent pays its
   operator's OperatorLink fee when the operator holds no CADRE. Hosted runtime cadre-agents (:10730,
   deploy/systemd/cadre-agents.service, keys AES-256-GCM at rest under /etc/lode/keys/agents/master.hex,
   sealed upload/export X25519+HKDF+AES-GCM, operator-signed create/spec/pause/resume/withdraw/
   export/delete); self-hosted runner bin/cadre-agent.ts. cadre-api routes/useragents.ts (tx/encode
   for Solana-wallet signing, builders register-agent/transfer/sol-transfer/spl-transfer/sol-memo,
   /v1/useragents/{a}(/card,/history), /v1/operators/{a}/agents, /v1/hosted/* pass-through via
   CADRE_AGENTS_URL). ~/cadre-web /agents/new wizard, /agents/mine(/[a]), operator card on profiles.
   Tests: agents 18, cadre-api 56 (7 new), cadre-web unit 28 (7 new).
   DEVNET WALKTHROUGH 2026-10-21 (headless Chromium, our test user key keys/devnet/useragents/user-test.json
   FkKLr1WNTHUtBdXCMShVqAcX7AppSd9oEkoFQq6fjMqo, runtime + cadre-api + site run on the Mac at
   10730/10731/10732 against the public devnet chain and Solana devnet): agent
   9CDcU57w5vejND297HBjUvfxU58TuCHaaq7hJFwakaL7 (@tide_iax9fp); SOL funding 311MPrVc4hDu...; CADRE
   Transfer 8f95e401... (block 4339, signed by the test wallet's signMessage); SetProfile 8f7b3f5a...
   and OperatorLink df182250... (block 4341); RegisterAgent 31c8dc1c... (4343); x402 top-up order 34
   (ticket d65887c9..., BridgeUnlock f8c9da29... at 4349); memo checkpoint on Solana 36aSEDJLqjsj...
   anchored by receipt post 9f4428be... (4353); paid thought req_ec0095707b8eb3677bc847de; verified
   on Cadre (all 8 checks); pause/resume via signed requests. e2e/agents-devnet.mjs 15/15 PASS
   (the run then stopped at a screenshot on ENOSPC: the Mac disk filled, partly from this row's
   scratch copy of node_modules, removed), e2e/agents-screens.mjs 28/32 (the 4 FAILs are the
   browser logging cadre-api's 503 when the Mac's IP gets 429 from the public Solana devnet RPC).
   That Mac-hosted agent was then withdrawn to the operator on devnet (SOL transfer HcKp2CyDkdDt...,
   receipt post 6cc25f1d..., Cadre Transfer e3af8d8e...) and deleted from the Mac runtime.
   LIVE ON DEVNET BOX 0 since 2026-10-21 16:51 UTC: cadre-agents.service (:10730, master key
   /etc/lode/keys/agents/master.hex, state /var/lib/lode/agents), cadre-api restarted with the drop-in
   /etc/systemd/system/cadre-api.service.d/agents.conf (CADRE_AGENTS_URL), site redeployed from
   cadre-web main: https://cadre.64-95-12-116.sslip.io/agents/new, /agents/mine,
   https://api.64-95-12-116.sslip.io/v1/hosted. Second walkthrough fully on the public URLs: 18/19 PASS
   (the FAIL is the browser logging cadre-api 502/503s from box 0's public Solana RPC 429s): agent
   FiL2yraRKYadA9KL8Po7PnP5Lh7tGsGEcr1h5gJgcge (@tide_icsp7l), SOL funding 2o6N8ZEpXRzA..., CADRE
   Transfer 2130336f... (4430), SetProfile 848808a3... (4431), RegisterAgent d1f2ef09... (4433),
   OperatorLink f1a13f5b... (4435), verified 8/8. It keeps running on box 0; its x402 top-up retries
   while box 0's IP gets 429 from the public Solana RPC (the dedicated RPC URL, SESSION-HANDOFF
   "Waiting on the owner" 1, fixes it). Not deployed: user-agent desktops (wait for the desktop
   controller to host user agents); npm publish of cadre-agent (owner's npm account).

24. FRESH NODE SYNC STALL FIXED (2026-10-21, "fresh node p2p sync stall" row):
   - Symptom: a node with empty storage joining the public devnet stopped at height 1-2 ("orphan pool
     full"); the NODE.md recipe had synced 3,022 blocks on 2026-10-20.
   - Root cause: chain length, not a commit. Headers-first sync walks back from a peer's tip and asked for
     every body as soon as its header arrived (chain/bin/agent-node/src/node.rs), so bodies came newest first
     and waited in the orphan pool (`MAX_ORPHANS` 4096, chain/consensus-pow/src/chain.rs) for their parents.
     Past 4096 each body was refused while the resolver counted it delivered and never asked again, leaving a
     gap above block 1. Devnet passed 4096 blocks on 2026-10-21. `MAX_HEADERS_ONLY` (16384, each entry with a
     ~25 KB proof) would have done the same at about 16k blocks.
   - Shown: a length-only test (fallback blocks) at b54e0c9 syncs 3000 of 3000 and stops at 1 of 4350; main
     before the fix stays at height 1 in the regression test; a box-built main synced live against devnet
     stayed at height 1 with 3 peers. (b54e0c9 cannot follow today's devnet: pre-3260/4200 upgrades.)
   - Fix (main aa5db01): the chain keeps a height-ordered queue of header-only entries;
     `Core::schedule_bodies()` (chain/rpc/src/core.rs) after every node event asks for at most `BODY_WINDOW`
     (512) bodies at a time, lowest first, never more than 512 above the tip; a body refused for a full
     pool is queued again. Header-only entries keep the 256-byte header only (the proof comes with the body
     and is verified when it connects), so `MAX_HEADERS_ONLY` is 262144 (about 80 MB at the cap). A node no
     longer serves a header it has no body for. Not a consensus change; old and new nodes mix.
   - Test: chain/bin/agent-node/tests/fresh_sync.rs `fresh_node_syncs_devnet_history_past_both_upgrades`
     (devnet chain id; 384-matrix RegisterModel; social kinds refused at 3258, SetProfile, OperatorLink,
     AdminConfig key 22, Post, reply, React with tip, Follow from 3260; a 640-matrix RegisterModel (25,815
     bytes) refused at 4199 and mined at 4200; tip 4350; a fresh node joins both seeds and matches tip,
     finalized hash, profile, handle, post, follow counts, params, both models and a balance).
   - Live: agent-node built from the fix, fresh data dir on the test box, devnet seeds only: 4352 blocks
     in under 80 s (389 MB data, 166 MiB RSS), tip 3894a69f..., finalized 4322 3ac10d0a..., equal to all
     three devnet nodes, 3 peers, no "orphan pool full", no bans. NODE.md recipe re-run from a fresh public
     clone of main (2ccfba4, `--locked`): build 555 s, 4,402 blocks in about 50 s, equal finalized hash.
   - Devnet boxes: run the fix since 2026-10-21 ~19:05 UTC as part of fix3 (below).
   - FOLLOWER STALL + MEMORY BLOWUP, ONE CAUSE (found 2026-10-21 18:10-18:40 UTC on the test box,
     ~/follow-stall): a node missing many blobs (devnet references about 2,200 model and job blobs,
     ~4.9 GB) kept up to 1024 blob fetches in flight with a 2 s resolver timeout; answers of up to 4 MiB
     could not arrive in time, so every key was asked again about every 2 s (logged: 77,974 timeouts in
     2.5 min over 1024 keys). Each peer read every asked blob in its node loop and queued it into that
     peer's single send queue, bounded at 1024 FRAMES (not bytes). Effects: (1) the follower's peers
     dropped its block, tip and header traffic behind the blob answers, so after a missed block (a
     one-block fork at 4515 here) the parent header never arrived and the node stopped following the
     tip while every peer listed it as connected; (2) the serving boxes held gigabytes of queued answers:
     box 1 (2 GB) OOM-killed 18:33:58 UTC, box 0 6 GB RSS. My two blob-enabled test-box followers were the
     flood; they stopped 18:35:58 and box RSS went flat. A node with the default 1 GiB blob quota also
     re-fetched forever what it could never store (the 10-block prune pass wanted it again).
   - Fix (main 0c57977..ba9f406, rolled out as agent-node fix3 6661bb6b... built from branch follow-stall
     7c641c8): open net send path has a bulk lane per peer for the blob channels (`register_bulk`), drained
     only when the normal lane is empty, with byte budgets `bulk_peer_bytes` 16 MiB and `bulk_total_bytes`
     128 MiB (over budget: dropped and counted in getPeers stats `bulk_dropped`); inbound blob frames are
     dropped (`bulk_in_dropped`) instead of blocking the connection's reader when the consumer is full
     (backlog 64); at most 16 blob fetches in flight (rpc/src/blobfetch.rs, stale after 5 blocks: cancelled
     and queued again), blob resolver timeout 30 s; the node loop's tip timer is the first branch of its
     biased select (it was starved: no tip announcements, `peers` 0 for minutes after a restart); a full
     blob store stops wanting blobs (follow-up commit). getBlobStats `fetch_in_flight`, getPeers
     `queue_dropped` / `bulk_dropped`.
   - Tests: net tests/open.rs `normal_lane_is_not_held_behind_bulk_frames` (without the lane the normal
     messages waited 52.9 s behind blob frames) and `bulk_queue_bytes_are_bounded_per_peer_and_in_total`
     (2 GB offered to 3 peers over 1 MB/s links stays within the budgets); rpc blobfetch unit tests (2000
     wanted, at most 16 in flight, a fetch nobody answers gives its slot back); chain workspace 164/164 on
     the test box on main with contracts merged.
   - Devnet rollout (run by the orchestrator, agent restarts are blocked): cadre-node only, box 1, 2, 0
     around 19:05 UTC, agent-node.pre-fix3 kept on each box; after each, equal tip and finalized hash
     (4599 149d4ca0681f, 4 peers); RSS about 1 min after restart 153 / 196 / 171 MB. Rollback:
     `cp -p agent-node.pre-fix3 agent-node.new && mv -f agent-node.new agent-node`, restart cadre-node.
     The contracts row then rolled main bc4a6ef (contracts + this fix + the full-store check) over it
     19:32-19:41 UTC. Soak: two fixed followers on the test box held the tip 19:14-20:15 UTC with 0
     dropped frames; devnet agent-node RSS 178-244 MB and flat through 20:13 (TEST-REPORT).
   - TO THE SECURITY ROW (uncommitted buffer budgets in chain.rs/lib.rs/core.rs): merge onto this; a dropped
     orphan or header entry must go back into `body_queue`, the next tip gossip does not re-ask for it.

25. HOUSE DESKTOPS (owner 2026-10-21: 20 house agents on live E2B desktops, then TWO per agent,
   Social + Work; hybrid brains; 25 USD/day cap; "house desktops" row): LIVE on devnet since
   2026-10-21 16:33 UTC. Spec + measured costs: ECOSYSTEM 2.10; code services/swarm/desktop
   (README); API docs/API.md "House desktops"; ops docs/DEVNET.md "House desktops".
   - What runs: `cadre-desktops.service` on box 0 (own checkout ~/cadre-desktops, 127.0.0.1:10720)
     drives 6 agents x 2 desktops = 12 E2B sandboxes (template `cadre-desktop`, 1 vCPU / 1 GiB):
     ferry_nan, cipher_kit, chef_ottoline, bridge_keeper, star_watcher, courier_rex (the last 6
     personas; cadre-swarm runs the first 24, SWARM_AGENTS=24). The house swarm decides and writes
     every word through the paid gateway; the agent's Social desktop signs it in the Cadre Wallet
     through the site, operated by `anthropic/claude-haiku-5.5` via OpenRouter (the Anthropic key
     in ~/.config/lineage/model.env has no credit). Work desktops are read-only until Phases 2-4.
   - Site: /live (grid) and /live/<handle> (both desktops, action log, thoughts with receipts) in
     ~/cadre-web c974e4b + 42ff6db; town deep links ?thought= ?reply= ?post=. cadre-api
     /v1/desktops (stream tokens, HLS proxy, stills). Deployed by the user agents row's 16:45 UTC
     window (sync + cadre-api restart + site).
   - Measured: 0.144 USD per agent hour (2 desktops) at the production read-only cadence; about
     0.149 with chain actions; 6 agents about 21.5 USD/day; all 20 x 2 about 71.6 USD/day. E2B plan
     is Pro (100 concurrent, 24 h; 40 at once tested): no upgrade needed, the cap is what binds.
   - Live proofs (probe account F2DnG5...GfVN, funded 0.001 CADRE from ops): a paid thought, then
     the probe's own desktop connected the wallet and posted it; proof 4 PASSES end to end (post at
     height 4451, body equal to the completion, thought_ref verified, receipt link;
     services/swarm/desktop/PROOF-LAST.json). `node src/proof.ts` repeats it. First house agent
     action from its own desktop: courier_rex's reply at height 4474 (17:25 UTC), thought_ref
     verified (15 vision steps, 0.0099 USD incl. the one-time wallet connect).
   - Content quality is the swarm's, not the desktops': the 1B model often echoes the quoted
     context, so many replies are filtered before any desktop task (no GUI cost), and one passed
     reply was a row of dashes. Better text needs the 8B on GPU or prompt work in services/swarm.
   - Kill switch: `scripts/devnet-desktops.sh stop` (STOP flag + pause all; billing stops);
     `resume` then `devnet-deploy.sh ctl 0 restart cadre-desktops`. Day cap pauses everything at
     25 USD (ledger /var/lib/lode/desktops/spend.json, E2B at list rate + OpenRouter usage.cost).
   - Open: OpenRouter balance is shared with x-agents (156 USD left at about 14:30 UTC, 59 USD that day
     used by others): top it up or give the desktops their own key. More agents need the owner to
     raise DESK_USD_PER_DAY (DESK_AGENTS=20 needs about 75) and cadre-swarm SWARM_AGENTS=10.

26. VERIFIABLE AI ANSWERS: cadre-glue-v1 DETERMINISTIC GLUE + FULL TICKET REPLAY (2026-10-21, "verifiable AI:
   deterministic glue replay" row; SPEC 4.7, docs/PROOFS.md row 11, research/VERIFIABLE-AI.md, EVALUATORS 4):
   - Chosen: (a) deterministic glue. Integer fixed point across reductions, single binary64 ops elementwise,
     integer exp/cos/sin tables. TOPLOC-style tolerance (b) is unnecessary once (a) holds; bonds/slashing (c)
     are designed on top (SPEC 4.7), not built (chain/ is under the Phase 1 / matrix-limit rollouts).
   - Code: tools/cadre/glue.py (the worker's default; `--glue fp32` keeps the old path); crates/cadre-glue (an
     independent Rust implementation: forward, replay, cadre-fraud-v1, byte-level decode, `cadre-replay` CLI);
     services/attester replay.rs (full ticket replay inside check_ticket; a mismatch is a RejectResult plus
     `GET /proofs/<ticket>`); cadre-gateway ledger + response blob carry the replay bundle (prompt_ids,
     output_ids, max_new_tokens, stop, glue).
   - Measured: PPL 16.1770 vs 16.1694 fp32 glue (+0.05%), tasks within noise (A40, 40 x 1024). A40 vs Xeon:
     0 of 144 matrices differ, logits of 3 full windows bit-identical; fp32 glue: 126-144 differ, 2 of 4
     answers changed. Speed: decode 2.8x slower on GPU (7.2-8.4 vs 19.2-24.7 tok/s, launch-bound), 1.5x on CPU.
     Replay: a 96-row, 144-segment ticket accepted in 19.2 s on the loaded test box; the replay's own cost is
     the glue (~15% of the GEMM check that already existed).
   - Found and fixed: torch's CPU float64 sqrt (SLEEF) is not correctly rounded (1 ulp in 5 of 1024 rows);
     CPU sqrt now from numpy, and the spec requires correct rounding. gemm_bt slice bug for n < threads.
   - GPU: one A40, 15:49-16:55 UTC, $0.66, terminated (API: no pods).
   - LIVE ON DEVNET (round 2, owner 2026-10-21 "turn it on"): box 0 deployed by the orchestrator 18:34-18:40 UTC
     from main 154a819 (cadre-gateway 41a51d8f..., pearl-attester fd5fc6d6..., worker restarted on the final
     rules; cadre-node restarted to reclaim 6 GB). First live paid ticket ecb7a659f7e4f1f0143b79068906fcd01df4c5eaa41ae6339943d4f8e20149e0
     (Order 36, "Paris", 48 rows): response blob carries the bundle, the box 0 attester replayed 144 segments in
     17.8 s (GEMM 8.8 s) and signed, CompleteJob a6be89db..., status completed. Settled on Solana by proof: cadre-settle "finalized order 36 by proof against final header 4560", tx 4YkF9ABPWpdxRwzXUCYF3Rm8x5k64YR7YHpkgV1cwBzQ3A5qBV8Uq4ZnurbGpckGZrvGR9UYV6muLSFLACBVtPS9 (2026-10-21).
     Mining of its 144 segments is slow: 11 mined by 19:20 UTC, still 11 at 02:40 UTC (the script's 8 h wait ran out); the miners on all three boxes were mining other tickets' 1004-row segments (chain at ~1 block/min, one segment per block), so this is segment-queue throughput, not the replay (open question for the mining / 8B rows: job selection order across tickets).
     House swarm: pending first thought after 00:00 UTC on the new stack (it hit its own SWARM_ROWS_PER_DAY
     16384 budget at 17:30 UTC, before the rollout; D1 unchanged; the orchestrator checks the post-reset
     thought). Its ticket open across the rollout mixes old requests, so it gets the GEMM check only.
   - NOT DONE / NEXT: (1) the test box demo stack (down since the 16:26 UTC reboot) runs cadre-glue-v1 when it is
     next started. (2) Tickets opened before 18:35 UTC close with the GEMM check only; every later ticket is replayed. (3) Bonds and slashing txs (BondServer / SlashServer / ReportFalseSlash, SPEC 4.7). (4) Faster
     GPU glue: fused kernels, the KV-head loop. (5) An 8B check (head_dim 128 path is specced, not run).

27. SERVE THE 8B: UPLOAD ORDER, TICKET BLOCK BUDGET, JOB CHECK AT MINING (owner-approved 2026-10-21, "serve the
   8B" row; follows item 21; no GPU, no 8B on devnet):
   - Upload (SPEC 4.5 "Upload order"): `pearl model register` uploads the manifest, sends RegisterModel and
     waits, then uploads the chunks (`--parallel 4`, byte-range reads); a registered manifest pins its chunks
     on arrival, so the upload may outlast `grace_blocks`; a rerun resumes. No consensus change.
   - Ticket budget (SPEC 5.4 "Ticket block budget"): `max_blocks = max_rounds x matrices x blocks_per_segment`,
     rounds from the deadline, round period and rows, cut to `min(--max-ticket-blocks (10,000, = the
     storefront's initial max_blocks_cap), max_job_blocks)`; the gateway never posts more rounds than that
     (last round admits only what it can stack, then the balance completes). 8B: 15 rounds at 10,000.
   - Light client (SPEC 5.3 "Job check at mining"): new instruction `begin_verify_job` proves the posting
     tx inline when a job block is verified; header-relay no longer registers posted jobs (indexes them,
     holds posting headers open until their jobs' deadline, restores a closed one from the accumulator),
     keeps the security pass's replace_job for the oversized-tx fallback. Before (devnet, measured): 0.0022
     SOL per POSTED segment (register tx + 2,240,280-lamport JobCommit, never returned), 1.437 SOL per 8B
     round; after: nothing per posted segment, no rent per mined one, same tx count per job header.
   - Found in the e2e and fixed: block templates scanned only the newest 512 jobs, so the oldest segments
     of a 640-segment round were never offered; now `TEMPLATE_JOB_SCAN = 4096` (node policy, chain/
     consensus-pow). Devnet nodes still run 512 (fine for the 1B) until the next node rollout.
   - E2E (test box, ~/serve8b, ports 11600-11665 via DEMO_PORT_OFFSET=1000, demo steps 1-6 + 11, synthetic
     Llama-3.1-8B-shape model: 640 matrices, k 2048, n 8; scripts/serve8b/): PASS. Registration via node 2
     throttled to 64 KiB/s with a 20-block grace: 1,280 chunks (10.5 MB) in 102.7 s, all held 1,427 blocks
     later. One paid round (256-row balance, x402 25,707,635 lamports, 2 requests, 32 rows): 640 segments
     posted in blocks 381-426 (20 batches of 32 txs), all 640 mined (382-1796, the stall before the template
     fix included), ticket completed, Order finalized by finalize_order_proof, 2,208 s from payment to
     settlement (a rerun on the tree merged with main, template fix from the start: 7/7 PASS, segments mined
     in blocks 340-979, settled 1,000 s after payment). Light client over the whole first run: 1,572 headers, 641 begin_verify_job (87.8k CU average),
     0 register_job, 0 JobCommit accounts, 61,662 txs, 0 failed, 0.308 SOL fees, lag at most 21 blocks.
   - Devnet: lightclient 47mav... upgraded 2026-10-21 (slot 513580446; .so 665,536 B sha256 2017132c...;
     programdata 718,124 B so no extend; buffer rent 3.38157312 SOL held during the deploy and returned;
     deployer 43.961159746 -> 43.957719746 SOL, 0.00344 SOL in fees incl. one failed buffer write closed
     and redone). header-relay rollout: see DEVNET.md "Header relay build".
   - Open for an 8B: the GPU worker (item 20), an upload node with >= 10 GB blob quota and a faster put rate,
     raising the storefront max_blocks_cap for long-lived 8B balances, the node rollout with the template fix.
28. SECURITY + ROBUSTNESS PASS BEFORE MAINNET (owner 2026-10-21, "security + robustness pass" row): DONE. Write-up in
   docs/SECURITY-REVIEW.md (all areas, open items by category; exploit details only in gitignored docs/security-private/).
   - Programs: reserve/storefront R-1, R-2/S-3, S-1, S-2 (d60c9d4); bridge B-1, B-2 (b4ce4db, relayer must roll out with
     the bridge upgrade); lightclient L-1, L-2 (22c739f) + header-relay fill_final_gap / replace_dead_job (5deedec).
   - Chain: property tests (0ecf5bf) + C-1..C-12 (9277119, merge 060dfd5); need a cadre-node rollout to take effect.
   - Services: V-1..V-6 (e23ca64, 9cd45aa, e976e3f, 88ad2de, 52247dd, ec2c80f), V-2b/V-8/V-9/V-10 (9bae779); deps:
     bn.js 5.2.5 (6ed714e), pearl-cli rustls-webpki/webpki-roots (a69323a). Wallet W-1..W-10 in pearl-wallet d3b3f05d0.
     Ops: d8bf302.
   - Redeployed 2026-10-21 (owner "Yes, allow deploy restarts", run by the orchestrator): test box cadre-api; box 0
     synced to 154a819, cadre-gateway + pearl-attester installed and restarted, pearl-miner rebuilt (sha256 295bfca2...)
     and restarted, cadre-explorer + cadre-api restarted (public API 200); boxes 1-2 the same pearl-miner (.prev kept),
     cadre-miner restarted.
   - Round 2 (2026-10-21): V-7 gateway front (64 in flight / 503, 30 s socket read timeout / 408, 413 over 4 MiB;
     24a5ff3) + crates/Cargo.lock webpki bumps; every gateway client on cadre-auth-v2 (pearl-sol 6a5294a, cadre-web
     2428699, pearl-wallet f7753402b on dev + danishseal cadre-chain); live v2 check on devnet 200 / wrong gateway 401.
   - Deployed (orchestrator): box 0 at bd38bfc, cadre-gateway 8df7edd6... (.prev kept), cadre-swarm, cadre-agents,
     cadre-desktops (0 auth errors), site from cadre-web 2428699; v2 check right 200 / other 401 / v1 200.
     devnet-deploy.sh sync + site now refuse when local main is not origin/main.
   - OPEN: a wallet release with f7753402b, then CADRE_AUTH_REQUIRE_AUDIENCE=1 on the gateways (box 0, test box); chain
     C-1..C-12 wait for the next cadre-node rollout; admin keys (AdminConfig, bridge set_signers/set_release_modes, lightclient set_params) need
     multisig + timelock before mainnet; attester/evaluator messages carry no chain_id (fresh mainnet keys).
29. CADRE CLI: STATUS, PING, TXS AND AGENTS IN WORDS (owner 2026-10-21 "a CLI that pings everything", "Cadre CLI"
   row): DONE. crates/pearl-cli now builds two binaries from one library: `cadre` (the product name; no command = `status`)
   and `pearl` (unchanged for scripts; no command = the overview box, the old `status` is now `overview`). New, all
   read-only: `status` (one screen, PASS / WARN / FAIL / SKIP over 3 nodes with finalized-hash agreement, the 4 Solana
   programs, mint, relay payers, light-client lag, cadre-api, explorer, storefront, relayer, gateway, site, settle,
   verifiable AI, house swarm, desktops, user-agent runtime, buyback / reserve / supply / orders, mining rate and paid
   share, bridge invariant; `--local` adds box 0's 127.0.0.1 services; exit 1 on FAIL, `--strict` on WARN; `--json`,
   `--health`), `ping <target> [-c N]`, `tx` (Cadre hash or Solana signature, every kind in words incl. social, DMs,
   contracts), `account`, `block`, `agent @handle`, `agents`, `job <id>`, `ticket` (+ replay verdict), `verify
   [--run]` (the evaluator's own check_ticket, read-only), `model`, `watch`. scripts/install-miner.sh `--cli` (default
   unchanged); scripts/devnet-health.sh adds the CLI's lines when /opt/lode/bin/cadre exists (NOT installed on box 0:
   needs a build there + the health script reinstalled; no restart). Docs: docs/CLI.md "Health and lookups",
   ~/cadre-docs guides/cli. Tests 56 (mock network in src/mock.rs). Live: TEST-REPORT "Cadre CLI".
   Found live: box 0's IP is rate limited by the public Solana RPC (cadre-api not ready, relayer unhealthy for seconds
   at a time, buyback / reserve / supply missing from the summary): the dedicated RPC URL (/etc/lode/solana-rpc-url,
   absent) fixes it, and `--local` uses it when present; the house swarm used 15,706 of its 16,384 rows by 20:00 UTC
   and went 2 h without a house post before posting again.

29. ECOSYSTEM PHASE 2: ON-CHAIN DMs (M2.1-M2.4, 2026-10-21, "ecosystem Phase 2: on-chain DMs" row; SPEC 4.10,
   API.md "DMs and channels", RPC.md "DM reads", cadre-docs protocol/messages):
   SUPERSEDED 2026-10-22 (item 42): DMs moved to Solana; DM pay is SOL. Group channels were not carried over.
   - Owner decisions: DMs on the Cadre chain (each message a tx); D4 sealed (X25519 + XChaCha20-Poly1305) for
     human-agent, public for agent-agent (client defaults; the chain takes both); D5 priced cold DMs with escrow,
     refund if unanswered, 100% to the agent (`dm_protocol_bps` key exists, 0); every DM parameter is AdminConfig.
   - Chain (main): kinds 16 SendMessage (with thought_ref), 17 ReclaimMessage, 18 SetConsent, 19 ChannelConfig,
     20 ChannelMember, 21 ChannelKey (agent-types/executor/state/rpc dm.rs, separate files); state tags 26-35;
     DM params keys 35-42 in their own record `["dm-params"]` (43-44 reserved), so the config record and the
     contracts tiers never change; escrow counted by the supply invariant; sendTx runs a next-nonce DM through the
     executor on the tip so refused DMs never enter the mempool; getAccountTxs roles `message` / `member`. Devnet
     activation DEVNET_DM_HEIGHT = 4800. Vectors chain/agent-types/vectors/dm.json + dm-crypto.json (Rust reference
     tests/dm_crypto.rs; the explorer, cadre-api, swarm, extension and wallet codecs and crypto check against them).
   - Services (main): apps/explorer/src/dm.ts index (threads, boxes, escrows and how they settled, consent,
     channels, key epochs); cadre-api routes/dms.ts (inbox, threads, messages, escrows, consent, channels, keys,
     SSE stream, 6 builders, /v1/tx/encode DM kinds; set-profile takes enc_key and the DM settings); services/swarm
     dmcrypto.ts + dms.ts (house agents publish enc_key + priced inbox, answer paid escrowed DMs first as one paid
     thought each, same mode as the incoming DM, decline with a refund when over budget or out of time; dm_price =
     the pump quote of 2 x rows per answer x lamports per row, kept when an input is unavailable; README "DMs");
     user agents answer when actions.reply is true, inside their spec budgets.
   - Site ~/cadre-web d14cd9d (main): /inbox (Primary, Requests, Blocked, Channels, sending lane, escrows),
     /inbox/[peer] (gate and price before signing, sealed toggle, metadata note, escrow countdown, reclaim and
     decline, block / allow / clear, Enable sealed DMs), /channels, /channels/[id] (joins, sealed epochs, admin
     keys); sidebar Inbox, profile Message button. Wallet ~/pearl-wallet bf7f2113e (dev + danishseal cadre-chain):
     codecs, dm-crypto (@noble/ciphers 1.3.0), window.pearl.dm (encKey, seal, open, channelKeys, channelSeal,
     channelOpen; per-origin "Read sealed messages" permission), approvals in words, profile DM settings, Inbox.
   - Tests: chain workspace 184/184 on the test box (executor dm 13 incl. policy matrix, escrow release/refund
     timing, decline, limits, protocol share, sealed framing, channels and key epochs; node RPC dm_rpc 2 incl. reorg
     rollback; dm_crypto 2); explorer 60, cadre-api 59 (+9 skipped), swarm 41, agents 20, extension tx-vectors 67,
     wallet unit 280 (+6 skipped; 2 pre-existing token IDL drift fails), wallet UI 132/132 headless, site unit 56,
     e2e 63/63, 68 headless screenshots (desktop 1440 / phone 390, light / dark). Live on the test-box DM chain
     (one agent-node, chain 1347616769, ~/dms-build, ports 11730/11740): cadre-api live check 43/43 (every route
     and builder with real signed txs, heights 1301-1331); swarm proof: paid plain DM answered and released
     (1361-1362), sealed DM answered sealed and opened by the sender (1362-1364), over-budget DM declined with a
     refund (1407-1408).
   - DEVNET NOT LIVE YET: the node window was refused by this session's permission check (20:10 UTC). agent-node
     5dcab264... (main e7e6cbb; DMs + contracts + fix3) is STAGED as /opt/lode/bin/agent-node.dms on boxes 0-2; a
     fresh test-box follower with it replayed devnet from genesis to tip 4653 with tip and finalized hash equal to
     all three boxes. To go live (whoever has permission): cadre-node only on box 1, 2, 0 (cp -p agent-node
     agent-node.pre-dms; cp agent-node.dms agent-node; systemctl restart cadre-node; tips/finalized/peers equal
     before the next box). It is safe after 4800 too: old nodes cannot decode DM txs, so none enter a mempool
     until all three run it. Then box 0 from main: devnet-deploy.sh sync, rebuild/restart cadre-explorer and
     cadre-api (index STATE_VERSION 3 re-indexes), cadre-swarm (optional env in services/swarm/README.md "DMs";
     check `node src/main.ts dm-price` first; the pump quote was 503 from Solana devnet 429s at 19:55 UTC),
     cadre-agents, then `devnet-deploy.sh site` (cadre-web main) and a wallet release from dev. Then a live
     headless check of /inbox on https://cadre.64-95-12-116.sslip.io with a test key.
   - Open: wallet Inbox has no channel screens (codec, approvals and window.pearl cover channels; the site has
     them); swarm letters between house agents are plain only and channels are not answered; send-message builder
     takes no blob_uri yet; the test-box DM chain (~/dms-build node + mine loop, cadre-api pid in
     ~/dms-api/cadre-api.pid) is left up for the devnet check and can be stopped by PID.

30. SMART CONTRACTS (owner 2026-10-21 "allow for smart contract builds"; docs/CONTRACTS.md, SPEC 4.9,
    "smart contracts" row): anyone can deploy WebAssembly contracts, run by wasmi 1.1.0 (interpreter,
    fuel-metered, pinned). Tx kinds 30-34 DeployCode / Instantiate / Call / Migrate / SetContractAdmin;
    config keys 27-34; state tags 40-48; an 89-byte contracts record (receipts_root, events_root) as a
    tx_root leaf before the coinbase receipt, so Solana proves events and receipts by inclusion
    (pearl_verify_sbf::contracts); contract storage under state_root (getContractStateProof).
    Rust SDK crates/cadre-contract-sdk + examples counter, escrow, token, bounty; `pearl contract ...`;
    RPC getContract, getCode, listContracts, listCodes, queryContract, simulateCall,
    getContractReceipt, getContractEvents, getContractStorage, getContractStateProof; explorer
    /contracts, /contract/<addr>, /code/<hash>, /tx/<hash>; cadre-api /v1/contracts..., /v1/codes,
    /v1/tx/contract-call; extension and Cadre Wallet decode the five kinds in plain words; cadre-docs
    "Contracts" tab. Devnet activation DEVNET_CONTRACTS_HEIGHT = 4700, inert until the first contract
    tx. Gas defaults sized from measurements (block 150M gas = about 1.1 s of the worst wasm found).
    Next: per-operator fuel table (wasmi 2.0), a Solana verifier for QMDB storage proofs, outside
    audit + host-ABI fuzzing before mainnet (CONTRACTS.md 12).
    LIVE ON DEVNET since block 4703 (2026-10-21 ~20:45 UTC): every node runs agent-node 327d167b... (main
    bc4a6ef; a fresh follower replayed devnet from genesis with it first, hash and state_root equal), relays
    run header-relay 4bb4f670... (devnet feature). Example contracts (our key keys/devnet/contracts-keypair.json,
    all immutable): counter DEgjVUmWddeiDduF2HpXR1z2rtXB7kLvyhvGb7YBAbgb, escrow
    ETyaKSLTxhsqZpqfX85rVTrM2Pt1B5j9svbiuxivXYng, token BfaxysW2erS1dPc37z4rsCQwobPrAknQ5A6q5o6P6aLB,
    bounty 23ruAcWqrGY2VVLAVuAswNDGCJnrTeneZyBmPFDwpXb6 (explorer /contract/<address>; docs/DEVNET.md "Contracts").


31. DESKTOPS PHASE A + SELF-HOSTED DENSITY TEST + FAST-LOOKING DESKTOPS (owner 2026-10-21; "desktops Phase A" row;
   docs/DESKTOPS-AT-SCALE.md sections 5.1, 9, 10):
   - Code (services/swarm/desktop): per-agent layout (`DESK_TWO_DESK_AGENTS`: the first N keep Social + Work, the rest
     one desktop) and the Phase A mode scheduler (one desktop, Social/Work every `DESK_MODE_SLOT_S` 1800 s, staggered;
     chain actions run on it in either mode with `thought_ref`); `DEFAULT_HANDLES` now all 30 personas from the end
     (a test checks no agent is driven twice for every DESK_AGENTS); per-desktop health every 300 s (stream, browser
     host, memory, last action; self-heal of the encoder / browser host; `/health`, `/status`, ops log check 7 in
     scripts/devnet-health.sh); stream settings `DESK_FPS/KBPS/SEG_S/STILL_S` (a change restarts each encoder once;
     playlist read prefetches segments); activity console on Work desktops (deskhost.mjs keeps
     `/live/<handle>/console` in front between vision tasks; `POST 127.0.0.1:10720/activity` for runtime events such as
     the fast executor's trades); `DesktopHost` with E2B and self-hosted (podman, src/selfhost.ts) backends,
     `DESK_SELFHOST_AGENTS` per agent. 12 tests (test box). cadre-api /v1/desktops: per-agent streams (one-desktop
     agents get one), `layout`, `health`, `stream`; 5 tests. ~/cadre-web: /live for 30 (1 s stills per tile, live
     video only for the focused agent, filters), /live/<handle> one-desktop view with health, /live/<handle>/console.
   - Measured: E2B 0.0733 USD per desktop-hour on box 0 (51 desktop-hours) -> 36 desktops 63.3 USD/day, cap 64.
     Density on the test box (5/10/15/20 desktops, 1 vCPU / 1 GiB each, 5-core slice): 0.13-0.29 cores and about
     590 MiB per desktop, 4.0 fps held at 20 even with the shared box 97% busy; vision 3.3-3.9 s p50 per call;
     ~33 desktops per 10 vCPU / 64 GB box at 15 fps -> about $1.06 per desktop-day all-in vs E2B $1.79.
     Recommendation: Phase A stays on E2B (two boxes would cost about the same); self-host Phase B and user agents.
     Stream: 15 fps / 800 kbit/s / 1 s segments / 1 s stills on 1 vCPU uses 0.28-0.34 of the vCPU on E2B, no
     throttling, browser CPU unchanged; viewer delay about 4 s (from 7-8 s); VNC would be 0.1-0.2 s (next step:
     view-only VNC relay, not built). Raw: selfhost/*.ts|sh in the repo; results in the section 5.1 / 10 tables.
   - Test box: podman + slirp4netns + passt installed (apt), image localhost/cadre-desktop-selfhost kept (2 GB) for
     reruns, /root/desk-density; slice removed, every container removed; `podman build` left netavark iptables chains
     for podman's own bridge subnet (no published ports).
   - Deployed by the orchestrator 2026-10-21 ~21:40 UTC (36/36 desktops). Follow-up the same evening (906e238 + the
     health age guard): two Work desktops' streams had been frozen for hours because the OOM killer took ffmpeg and
     every restart then refused to overwrite thumb.jpg (now `ffmpeg -y`, one loop per desktop); the rollout's own kill
     commands matched their shell, so old 4 fps encoders and old browser hosts survived (fixed: bracketed patterns,
     SIGKILL); heal now also restarts a frozen stream and the browser (dead, stuck, or under 60 MB free, never while
     busy), skipping a browser host younger than 180 s; Work desktops open 2 tabs + the console (Work memory free went
     from 65-77 MB to 144-160 MB). After the 21:56 UTC restart: all 36 at 15 fps, one encoder each, 36/36 streams alive.
     Measured 22:00-23:00 UTC: 0.0713 USD per desktop-hour (61.2 USD/day at 36), 36/36 streams alive.
     Then (orchestrator go-ahead): Work desktops at 1.5 GiB from their own E2B template `cadre-desktop-1536`
     (`DESK_WORK_TEMPLATE`, `DESK_WORK_MEM_MB=1536`; the 6 old Work sandboxes are replaced, Social ones kept; ledger books
     each desktop at its own rate, +1.17 USD/day, about 62.4 at 36); the browser host now reports what is in front
     (`~/.cadre/front`) and a vision task waits for its working tab (tasks had typed half a URL into the activity console
     or a Google search while the host switched tabs: most read_profile failures); model errors retried twice.

31. CX ADDRESS FORMAT (owner 2026-10-21: "Cx" + hex, EIP-55 style checksum, option A: the full
   address stays the 32-byte key; "Cx address format" row): DONE in code, docs and the site; node
   and service restarts pending (commands below). Spec + vectors: SPEC 2.1, vectors/cx-address.json
   (Rust chain/agent-types/src/cx.rs, TS packages/cx-address; checksum = blake3 over the lowercase
   hex). Example C1Epvwo9... = Cxa38030920a564589bDFe976BDB50f37CdeaECBBb34b3934e65b3f1a717bB7d06,
   short Cxa380...7d06. Inputs take Cx, lowercase Cx and base58 everywhere; JSON keeps base58 and
   adds `<field>_cx` (RPC.md "Addresses", API.md "Addresses"). Surfaces: node RPC (parse32 + one
   output pass chain/rpc/src/cxjson.rs, also WS notifications), cadre/pearl CLI (fmt::parse_chain_addr,
   cx_str, short_addr, JSON pass; entry screen card), cadre-api (src/cx.ts: inputs normalized to
   base58 before handlers, 400 bad_address with cx_code, `_cx` on replies except signable payloads),
   explorer (addr() view helper, agent page shows "Cadre address" + "Solana address (same key)"),
   swarm / user agents / desktops (log + status `_cx`, isAddress accepts Cx), ~/cadre-web 79016bf
   (CxAddr everywhere, inputs + routes take Cx), Cadre Wallet dev + danishseal cadre-extension
   cadre-chain 8bbabb0d0 (receive, send validation, approvals, window.pearl adds `cx`; Phantom
   path test with the "abandon ... about" vector), ~/cadre-docs b45446d (reference/addresses).
   Not converted yet: services/swarm/trading (not on main; trading row asked to use shortCx/toCx),
   CLI contract return values of type addr, the old extension/ prototype, agent-node `pubkey` output
   (scripts parse it). The RPC change ships with the NEXT cadre-node rolling upgrade (no window of
   its own); cadre-api already adds `_cx` itself, so nothing waits on it.

32. AGENTS TRADE ON CHAIN (owner 2026-10-21 "getting AIs to trade directly onchain ... look into what auton
   was"; "agents trade on chain" row): DONE and LIVE on devnet; plan of record docs/TRADING.md, Auton research
   research/AUTON.md. Solana mandate program (user-owned trading account, agent can only swap within on-chain
   limits, withdraw never blocked, fee on realized SOL profit above the HWM, 1,000 bps to the buyback vault)
   deployed at BG5WkxZyYwWXTY2YSAJGhwL7JcxChRXx8UqHEAMAC5yc (2.113 SOL); Cadre WASM AMM + mandate contracts live
   on Cadre devnet (an agent-launched token DESK with its CADRE pool, a Cadre mandate); services/swarm/trading
   runtime (paid-thought stance, in-process executor, risk rules, receipts, latency rows); cadre-api
   /v1/trading* + /v1/tx/mandate-*; site Trading tab, /trade, grant flow (cadre-web 96fc127); `cadre trades`;
   wallet mandate approvals (pearl-wallet e321e94c9). Tests: mandate LiteSVM 11/11, contracts 3/3, runtime 6/6,
   cadre-api 4/4, wallet 3/3. Measured: event -> signed order under 0.2 s; PumpSwap send -> confirmed median
   754 ms; Cadre send -> included median 54 s; paid thought 5.7-108.5 s on the shared CPU worker (TRADING.md 6).
   Three house traders (desk_maker, tape_reader, slow_hand) run on the TEST BOX (~/trading/run-trader.sh,
   kill file ~/trading/STOP). PENDING for the orchestrator: box 0 cadre-api restart + site redeploy, and
   optionally moving the traders to box 0 (deploy/systemd/cadre-trading.service), TRADING.md 10.

33. TRADING FOR EVERYONE: SOVEREIGN AGENTS ON SOLANA (Phase 4b, owner 2026-10-21 via the orchestrator:
   [SUPERSEDED for user agents 2026-10-22, STRATEGIES.md 6.1: user agents trade PRACTICE only; the real-swap
   engine below runs only with `trade.real_swaps`, which no creation flow sets.]
   "people ... create their own agents ... and start trading themselves", then: trades on Solana only;
   agents are their own bosses, "that applies for all"; "trading for everyone" row): DONE in code, LIVE on
   devnet from the test box. Agent spec v2 (SPEC 4.8): sovereign, fixed at creation, trade skill with hard
   bounds, evolve (agent_self). Hosted runtime refuses every creator verb on v2 agents (403 sovereign);
   legacy v1 agents keep the old controls, labelled legacy (none migrated). User agents run the house
   engine from their own wallets (services/swarm/trading src/wallet-venue.ts, src/useragent.ts); cadre-api
   user-agent fills, leaderboard user_agents (organic) apart from house, sovereignty fields, any pump mint
   on the pump builders. Site cadre-web 58cacc6 (v2 wizard with Trade + Evolve + trust box, read-only
   /agents/mine, /trade user vs house), CLI 6fd0031 (Trade view 8, cadre quote/positions/buy/sell with
   y/N), wallet de929c621 (any pump coin trade, receipt posts, v2 spec warning; dev + danishseal
   cadre-chain). Live: sovereign self-hosted agent tide_ledger 8715zmRX... on the test box
   (~/usertrade/run-agent1.sh; 4 own-wallet buys, receipts on Cadre); a person's CLI buy 5jUAxDes...
   Not built: agent token launches (no pump create_v2 builder). PENDING for the orchestrator: box 0 sync,
   `npm ci` in services/swarm/trading, cadre-api + cadre-agents restarts, site redeploy (TRADING.md 10).
   Test box leftovers: ~/usertrade (agent1 running, cadre-api 127.0.0.1:10780 pid ~/usertrade/api.pid,
   web/cli/wallet build trees, Xvfb :74 pid ~/usertrade/cli/xvfb.pid), ~/usertrade-keys (0600).

34. AGENT SERVICES MARKET (ECOSYSTEM Phase 3, M3.1-M3.3; owner-approved 2026-10-21, D6 2,000 bps, D8
   fee-only registration; "ecosystem Phase 3" row): DONE and LIVE on Cadre devnet; docs/MARKET.md. Built as
   a WASM contract (contracts/market), no new tx kinds: market E1kpbaHn14z4Amdqb7SGjLbn576q1KPC3haAG6YQdsym
   (code 1647ce24..., admin and fee_to the market ops key J4F39n..., keys/devnet/market, gitignored).
   Offers, escrowed hires, deliver, buyer-named evaluators (t of n), reject / decline / timeout refunds,
   fee = ceil(price x 2000 / 10000) kept only on completion. LLM judge: `pearl-evaluator --judge-market`
   (paid thought, rubric cadre-judge-rubric-v1, verdict + reasoning blob + thought_ref). House desk
   (services/swarm/market): scribe_wren, ledger_lark, verse_finch, tally_crow + judge court_clerk, house
   flag under the house operator, hires every 900 s inside 60 CADRE/day per buyer. User agents: opt-in
   actions.hire (limits.hire_units) and optional actions.offer (offers). cadre-api /v1/market* +
   POST /v1/tx/market (plain-words review); cadre-web /market, service pages, hire pages, my jobs,
   profile Services tab (cadre-web 87ce5a5); wallet decoding (pearl-wallet dev 603554249, also danishseal
   cadre-chain); `cadre market|offer|hire` + Market view 9 in the Cadre app; cadre-docs
   guides/market (3ab81ce). Live: hire #0 rejected by the judge (4940), #1 completed (4967), #2 through the
   builder path completed (4972); receipts verified (TEST-REPORT "Ecosystem Phase 3"). Desk and judge run
   on the TEST BOX ~/market-build (run-desk.sh, run-judge.sh, desk.pid, judge.pid, kill file
   ~/market-build/STOP; keys ~/market-keys/devnet). PENDING for the orchestrator: box 0 cadre-api restart
   and site redeploy (MARKET.md 11). Open: the contract cannot pay the mining pool (fee_to instead),
   no SOL hire via the storefront, no open bids (MARKET 12).

35. BATCHED INFERENCE WORKER + GATEWAY BACKPRESSURE + REQUEST DATA RETENTION (owner-approved 2026-10-21,
   "batched inference worker" row; SPEC 4.7 "Batching", SPEC 5.4 "Worker concurrency" and "Request data
   retention", MAINNET 7, research/SUPPLY-DESIGN.md 4.5; data in docs/measurements/batching-2026-10-21/):
   - Worker (tools/cadre/batch.py, the server default under cadre-glue-v1): continuous batching. One
     scheduler thread runs a forward over every active request's rows (decode rows + prefill chunks up to
     --max-step-rows), the int32 GEMMs stacked, attention per request (on CUDA one pooled, masked,
     zero-padded attention for all decode rows; KV pool of --max-batch slots), batched greedy pick, FIFO
     admission, --max-batch 16 / --max-queue 64 (one more: 503 Retry-After) / --max-live-rows 8192 /
     --flush-rows 128; each step records which rows are whose and a request's A/C rows go to its own
     files. --max-batch 1 or --glue fp32 = the old one-at-a-time worker. HTTP backlog 256 (the default 5
     reset clients at 64 concurrent, measured).
   - Chain: unchanged. Batching shares the worker's GEMMs, never a segment: each ticket still posts one
     segment per matrix per round with exactly its own rows, so per-segment accounting and rows charged
     per ticket are the same. No chain change was needed.
   - Bit identity (tools/cadre/test_batch.py): 9 mixed real requests (prompts 42-164 tokens, one longer
     than a prefill chunk, a stop string, pad on and off, 4 slots so 5 wait and join mid-decode, 2 late
     joiners): batched == alone for output ids, text, rows, finish, all 288 A/C files and every token's
     binary64 logits row, with and without the KV pool; test box CPU 28/28, A40 28/28 (twice), tiny
     synthetic model 17/17 (box and pod). The evaluator's Rust replay (cadre-replay local) accepted every
     batched request (7 unpadded of 9) on CPU and on the A40. CPU vs GPU: the A40's files equal the same
     pod's Xeon files byte for byte for all 9 requests. (A pod-quantized model differs from the box's
     registered one: calibration is not bit-reproducible across machines, so compare on the same files.)
   - Measured, A40 $0.59/hr, 64 prompt + 64 output per request, segment files written: one at a time 7.57
     tok/s ($21.65 per 1M), batched 12.2 (1), 72.5 (8), 99.9 (16), 140.0 (32, $1.17 per 1M), 130.6 (64);
     without segment files 318-321 tok/s ($0.51). Test box CPU (8 vCPU shared, load 3-6): 4 threads 2.50 ->
     17.49 tok/s at 16 (7.0x), 2 threads (box 0's) 1.71 -> 11.70 (6.8x); ttft with 4 queued 57-85 s one at a
     time vs 2.5-4.5 s batched. Tables: MAINNET 7. Pod 23:12-01:03 UTC, $1.08, terminated, no pod left.
   - Price (owner decision D2): recommended $2.34 per 1M rows (2x the batched A40 cost, was $3.86), key 25
     `price_mlamports_per_gflop` 8,767 at SOL 108.09 (was 14,736); 11.6x the cheapest market completion
     price (was 19x). SUPPLY-DESIGN 4.5. Nothing set on any chain.
   - Gateway: --max-inference 32 at the worker (503 overloaded, Retry-After 2, before rows are reserved or
     x402 is charged); a worker 503 is a 503 (reservation released). Retention (orchestrator priority after
     box 0's disk filled 2026-10-22 00:12 UTC with 109 GB of request dirs): a request's A/C dir is deleted as
     soon as its round is posted and booked; each settlement pass sweeps dirs of dead tickets and orphans
     (10 min grace); --max-request-data-mb 16384 caps data waiting for rounds (+ in-flight estimates) and
     --min-free-disk-mb 8192 keeps a disk reserve: a paid request past either answers 503
     insufficient_storage (Retry-After 30) before any rows are reserved or paid; above half the cap rounds
     post early (the 3 h big-balance period is what let 109 GB pile up). /v1/stats `inference`,
     `request_data`. Sizing: 2.7 MB per row; batching at box 0's measured 11.7 tok/s (2 threads) is about
     23 rows/s, about 62 MB/s of scratch, so the cap is reached in about 4.5 min of full load and rounds then
     post every pass instead of every 3 h; disk use is bounded by the cap, not the round period.
   - Tests: gateway 27/27 on the test box (new: backpressure 2, retention 3); test_cadre 9/9; the
     server_limits silent-client test fails on macOS on unmodified main too (Linux passes).
   - Memory (box 0's worker unit caps 4 GB): peak RSS at 8 concurrent 256-prompt requests, 2 threads:
     batched max-batch 8, step 64, live 2048, flush 32 = 3.00 GB; the one-at-a-time worker on the same
     prompts 3.37 GB; max-batch 16 (step 64, live 4096) 3.56 GB; max-batch 8 at step 256 = 4.76 GB (over the
     cap). deploy/systemd/cadre-worker.service now sets the 3.00 GB settings (CADRE_MAX_BATCH=8 ...).
   - BOX 0 ROLLOUT (handed to the orchestrator; this row restarted nothing). Window: one at a time, worker
     then gateway, health checks between; requests in flight at the worker restart fail with their rows
     released. From the Mac, main pushed with this item's commits:
       0. baseline: `sshb 0 root 'df -h /var/lib/lode; systemctl is-active cadre-worker cadre-gateway;
          curl -s 127.0.0.1:10665/health; curl -s 127.0.0.1:10664/v1/stats'` (if free disk is under about
          10 GB, add `--min-free-disk-mb 4096` to GATEWAY_EXTRA first, or every paid request answers 503)
       1. `scripts/devnet-deploy.sh sync`
       2. gateway binary only: `sshb 0 lode 'bash -lc "cd ~/pearl-sol/crates && cargo build --release -j 2 -p
          cadre-gateway"'` then `sshb 0 root 'cp /opt/lode/bin/cadre-gateway /opt/lode/bin/cadre-gateway.prev
          && install -m 755 /home/lode/pearl-sol/crates/target/release/cadre-gateway /opt/lode/bin/'`
       3. worker: `sshb 0 root 'cp /etc/systemd/system/cadre-worker.service /root/cadre-worker.service.prev &&
          install -m 644 /home/lode/pearl-sol/deploy/systemd/cadre-worker.service /etc/systemd/system/ &&
          systemctl daemon-reload && systemctl restart cadre-worker'`; health: `until curl -sf
          127.0.0.1:10665/health; do sleep 2; done` must show `"batching": {"mode": "continuous", "max_batch":
          8, ...}`; smoke: `curl -s 127.0.0.1:10665/generate -H 'content-type: application/json' -d
          '{"prompt_ids": [128000, 9906, 1917], "max_new_tokens": 4}'` answers output_ids
       4. gateway: `systemctl restart cadre-gateway`; `curl -sf 127.0.0.1:10664/health`; `curl -s
          127.0.0.1:10664/v1/stats` shows `inference` and `request_data`
       5. watch 15 min: worker /health `active`/`waiting`, `systemctl show cadre-worker -p MemoryCurrent`
          (under 4 GB), `journalctl -u cadre-worker -u cadre-gateway -n 50`, `du -sh
          /var/lib/lode/gateway/requests` falling as rounds post (`request_data.deleted` rising); house
          thought latency in the swarm logs
       6. after one posted round shows `deleted` > 0: `systemctl disable --now cadre-gateway-prune.timer`
     Rollback: worker to one at a time without a code change: `mkdir -p
     /etc/systemd/system/cadre-worker.service.d && printf '[Service]\nEnvironment=CADRE_MAX_BATCH=1\n' >
     /etc/systemd/system/cadre-worker.service.d/10-serial.conf && systemctl daemon-reload && systemctl
     restart cadre-worker`; old unit: `install -m 644 /root/cadre-worker.service.prev
     /etc/systemd/system/cadre-worker.service`; gateway: `install -m 755 /opt/lode/bin/cadre-gateway.prev
     /opt/lode/bin/cadre-gateway && systemctl restart cadre-gateway` (keep the prune timer then).
     Box 0's house swarm, desktops, trading and user agents share this worker: one at a time they queued
     (trading thoughts 5.7-108.5 s, mostly queue); batched, 8 decode together (box 0 itself not measured).
   - Open: the segment files are half the GPU cost (an in-memory hand-off to the gateway, C kept as its
     hash); the 8B batched (not measured); a GPU worker next to the gateway (MAINNET 11 item 10).

36. ECOSYSTEM PHASE 5: LEADERBOARDS, DISCOVERY, TRADER BOARD, LISTINGS PREP, BOT FEED (owner "proceed"
   2026-10-22; "ecosystem Phase 5" row; ECOSYSTEM section 4 Phase 5, docs/API.md "Boards", docs/LISTINGS.md,
   docs/CLI.md "Boards (view 10)", cadre-docs guides/boards):
   - cadre-api (services/cadre-api/src/boards/ + routes/boards.ts; src/shared.ts lets the boards reuse the
     trading scan and the market index of the same process): nine live boards from chain data only
     (trading_pnl, market_earnings, acceptance_rate, paid_dms, hardest_thinkers, most_tipped, most_followed,
     miners, evaluators) with windows 24h/7d/all and sides organic/house/all; self-dealing excluded by the
     explorer and market rules; house money in house_value; minimum samples; every row links its txs.
     /v1/leaderboards replaced the Phase 1 stub list (town.ts). Discovery /v1/discover/{agents,trending,new};
     trader board /v1/traderboard(/{metric}); event feed /v1/events, /v1/events.rss, /v1/stream/events.
   - services/feed-bot: Telegram / X poster of the feed, DRY RUN unless FEED_LIVE=1 + FEED_TARGETS + the
     target's credentials; deploy/systemd/cadre-feed-bot.service ships dry. Nothing was posted anywhere.
   - tools/defillama: mandate Settled fees (fees, supply side, holders revenue), breakdown methodology; live
     all-time check on devnet PASS against the storefront Config counters, cadre-api /v1/buyback and the
     Mandate accounts (TEST-REPORT "Ecosystem Phase 5"). docs/LISTINGS.md: token info, supply endpoints,
     DexScreener (lists pump.fun / PumpSwap on mainnet by itself; nothing on devnet), CoinGecko, DefiLlama
     PR steps, mainnet switch checklist.
   - CLI: `cadre top [board]` and the Boards view (view 10, key 0); cadre-web /leaderboards (+ /[board]),
     /discover, /board.
   - Live check: a branch cadre-api on the TEST BOX 127.0.0.1:10790 (~/boards/run-api.sh, pid ~/boards/api.pid,
     reads the public Cadre devnet node and the devnet Solana RPC from ~/boards/solana-rpc-url, 0600) and a
     receipt proxy 127.0.0.1:10791 (~/boards/gwproxy.mjs, pid ~/boards/gwproxy.pid) mapping /v1/requests/* to
     the devnet API's gateway pass-through. Stop both by PID after box 0 serves the routes.
   - DEPLOY (box 0, handed to the orchestrator; agents may not restart services): see the row and the
     final report of this row: sync box 0 to origin/main, restart cadre-api only, health checks, rollback to
     the recorded previous commit; then the site from main with CADRE_WEB; optional cadre-feed-bot unit (dry).
   - ROUND 2 (2026-10-22, node 0 RPC saturated after the boards went live): every listJobs call makes the
     node load every job (up to 10,000; 8,298 on devnet), and the explorer indexer ran a full paged scan (42
     calls) on every poll. Now: one RpcGate per cadre-api (CADRE_NODE_RPC_CONCURRENCY, default 3) shared by
     every route and the indexer, stats in /v1/ready `node_rpc`; the indexer does a full jobs scan every
     CADRE_JOBS_FULL_MS (cadre-api 600 s; explorer default 300 s), otherwise the newest page every
     CADRE_JOBS_POLL_MS (30 s) plus a rotating getJob refresh of 100 open jobs; the top block window is re-read
     only when the tip moves; boards inputs (market, trading) and the discover directory on 60 s timers;
     town and DM pending lanes fetch each mempool tx once (CADRE_PENDING_MS 5 s); market reads refresh at most
     every 30 s (CADRE_MARKET_REFRESH_MS). A/B on the test box (same site-like load, 180 s, node 2 behind a
     counting proxy): old 1,148 requests / 2,616 calls, 413 listJobs, peak 15 in flight, 136 ms average;
     new 514 / 1,136, 5 listJobs, peak 3, 12 ms. Other node 0 RPC load seen by tcpdump (60 s): public RPC via
     caddy (remote readers incl. the usertrade row's branch cadre-api on the test box, old code, full scans
     against node 0), cadre-miner listJobs 35/min, cadre-settle getJob, cadre-gateway getTx/sendTx.
   - Open: the registration bond (D8, not wanted), a "Rising" board, per-buy buyback txs older than the
     indexer's first poll are not recorded (counters link the Config account instead).

37. BLOB DATA LIFECYCLE: SEGMENT A RETENTION, QUOTAS WITH BACKPRESSURE, BACKLOG DRAIN (2026-10-22, "blob
   lifecycle" row; SPEC 4.5 "Segment data retention", SPEC 5.4 "Node blob backpressure", RPC getBlobStats):
   - Incident: node 0's blob store hit its 8 GB quota; every settle failed ("putBlob: blob store full"), request
     scratch piled up (109 GB, then 67 GB). The orchestrator raised node 0 to 30 GB and the gateway retention
     (item 35) stopped new scratch.
   - Root cause (measured on node 0 at height 5009): 6.48 GB of segment A, 5.67 GB of it for segments never mined.
     A segment stayed Open, so its A was kept until its ticket's deadline (balance tickets: 10,000 blocks, about a
     week). The chain mines about 60 segments an hour; house traffic posted about 290 an hour (rounds of 144),
     and templates offer only the newest 4096 jobs (devnet nodes ran 512), so most of that A could never be mined.
   - Rule (node policy over chain state; SPEC 4.5): keep A while (1) mining: mineable at the tip or at F, within
     the newest 4096 jobs and `segment_mine_blocks` (1440) of posting; or (2) replay (node.json `keep_replay`,
     default true): ticket live at F until deadline + W, closed at F until close + W. W = `segment_replay_blocks`,
     AdminConfig key 48, default 720, in its own record (tag 52; devnet activation 5800, only for SETTING it). Model
     blobs stay pinned; profile/post/DM/ticket/job blobs unchanged.
   - Backpressure: putBlob stops 5% under quota ("blob store nearly full ... RPC puts stop at N"); the gateway
     answers 503 insufficient_storage when the node's `rpc_headroom_bytes` cannot take the A of every waiting row
     plus the request's; an attester replay checks rebuilt A by commitment and fetches A only to locate a mismatch.
   - getBlobStats: `classes` (model, segment_a, social, ticket, job, other; count, bytes, kept, prunable),
     `referenced_bytes`, `prunable_bytes`, `rpc_headroom_bytes`, `puts_open`, `retention`.
   - Measured sizes (1B, 144 matrices): 231,424 bytes of A blobs per row (113 distinct inputs x 2048; gate/up and the
     16 lm_head shards share); scratch 2,708,480 per row; so a 8192-row ticket holds at most 1.90 GB of A, a
     2048-row one 474 MB. Drain: node 0's store grew 1.56 GB while 17.4 GB of scratch posted (8.9%). Traffic: about
     2,200 rows an hour, 20 live balances with 59,392 rows of budget (25,296 used), 1.04 min a block.
   - Steady state under the rule: replay node (box 0) about 15 GB typical, 26 GB worst case (model 1.79 + live
     tickets up to 13.7 + closed in the window about 6.1 + mining tail); mining-only node: model + newest 4096 jobs'
     A, measured 4.8 GB of A (simulated on the live state), at most 16 GB at any traffic. Recommended quotas: box 0
     40 GB (disk 145 GB: about 25 GB other, 16 GB scratch cap, 20 GB reserve), boxes 1-2 16 GB with keep_replay false
     (48 GB disks, 32 GB free). Boxes 1-2 need segment A only to mine (their miners are 2/3 of the hashpower) and keep
     only what is mineable; they were at 7.72 of 8 GB when measured.
   - Backlog: no ticket needed a refund (deadlines 14,300-15,000, rows reserved when served, the gateway sweep and the
     CompleteJob pro rata cover dead tickets); with node 0 at 30 GB it posts by itself (71.2 GB 02:07 -> 53.7 GB
     02:45 UTC, about 27 GB/h). Stopgap applied by the orchestrator (seen 03:14 UTC) so paid AI flows during the
     drain: GATEWAY_EXTRA `--max-request-data-mb 65536 --min-free-disk-mb 20480` (backup
     /root/inference.env.pre-drain); revert once `request_data.bytes_waiting` < 16 GiB.
   - Tests (test box, -j 4): chain 196 pass, 0 fail (new: bin/agent-node/tests/blob_lifecycle.rs 3 = retention by
     state + model pinned + stats, stats + put backpressure, reorg around a mined and a pruned segment + fresh node
     after pruning; executor/tests/blobparams.rs 2; blobs + state units); gateway 30 (new tests/node_blobs.rs 3);
     attester 20 incl. the real-model replay (honest Accept 12.1 s, altered A and wrong C rejected). Live: the staged
     agent-node synced devnet from genesis on the test box (5077, finalized hash 4038f7df... equal to box 0) with
     keep_replay false and a 600 MB quota: it fetched only segment A of mineable segments, getBlobStats classes
     added up; stopped by PID, data removed.
   - ROLLOUT (handed to the orchestrator; agents' restarts are blocked). Staged in /home/lode/stage-bloblife/ from
     main a72713b: agent-node 556739d3... (boxes 0-2), cadre-gateway 38271463..., pearl-attester 33c5b897... (box 0).
     The node change is not consensus (nothing sets key 48 until every node runs it and height >= 5800), old and new
     nodes mix, rollback is a binary swap. One box at a time, 1, 2, 0, health checks between. See DEVNET.md "Blob
     lifecycle rollout" for the exact commands.
38. CADRE OPEN ITEMS: MARKET FEE TO THE POOL, AGENT TOKEN LAUNCHES, KILL SWITCH (owner "proceed" 2026-10-22;
   branch cadre-launch). No chain/ change, no node rollout.
   - Market fee to the mining pool: a contract already pays INTO the pool with a native Transfer as
     itself (the `transfer` host function refuses reserved accounts, native Transfer refuses only the
     escrow), so instead of a new host function behind an activation height the SDK names it
     (`native::mining_pool`, `native::pay_mining_pool`) and contracts/tests/tests/pool.rs pins pay-in-only,
     the supply invariant and a dropped-branch reorg (CONTRACTS.md 6). Market code v2 pays the fee to
     the pool when fee_to is the pool. DEVNET: DeployCode 5040 (d85bb397...), Migrate E1kpbaHn... to
     426cd50d... at 5041 (39f87db5...), set_config fee_to = pool 6LpEAcFW... at 5042 (6ffe7501...);
     live hire #10 (our test keys) completed at 5106 (8b807af1...) with fee_to = the pool in the event.
   - Agent token launches: cadre-api POST /v1/tx/pump-create (create_v2 + dev buy in one tx, 1,232-byte
     check), GET /v1/launches, /v1/launches/{mint}, /v1/l/{id} and /v1/launches/metadata/{id}; agent spec v2
     `launch` section + skill (services/swarm/agents/src/launch.ts); trade_own with PumpCurveWallet
     (services/swarm/trading/src/curve-venue.ts); self-trades labelled and kept out of organic volume in
     /v1/trading. DEVNET test launch with our own key: mint HzqW62Pm... (tx 5fD8miC9..., TRADING.md 9).
   - Kill switch: contracts/halt at 88uaR3ea... (authority the chain admin key), read by cadre-agents and
     cadre-agent before acting (ECOSYSTEM 7.8); GET /v1/halt. Live halt and resume checked 5046/5047.
   - PENDING for the orchestrator: box 0 sync + restart cadre-api, then cadre-agents (TRADING.md 10,
     "Launches + halt"). Then optional: a fully agent-driven launch (a v2 test agent with the launch
     skill, self-hosted on the test box). The site wizard has no launch toggle yet (~/cadre-web).
   - FOLLOW-UP 2026-10-22 (after the box 0 deploy at 741194c): CLTEST's metadata 404'd because the blob
     lifecycle rule pruned the unreferenced blob. Fix: cadre-api POST /v1/launches/metadata keeps the bytes
     on its disk (<STATE_DIR>/launch-meta), GET /v1/l serves disk first and adopts node copies; agents store
     metadata there (no launch without it); CLTEST's exact bytes are in deploy/launch-meta/04772db1....json
     (re-put on the node as a stopgap). /v1/launches total 0 is right: CLTEST's creator 7pYUzH... has no
     Cadre profile, so it is not a user agent. Deploy: TRADING.md 10 step 2 again, then the restore curl.

39. GATEWAY STUCK NONCE: ONE REFUSED POSTSEGMENTS BLOCKED EVERY SETTLE (2026-10-22, "gateway stuck nonce" row;
   SPEC 5.4 "Server nonce" + "Settle order", RPC sendTx):
   - Incident (box 0, 04:23 to at least 12:56 UTC): rounds_posted stuck at 45; every settle failed with "send
     PostSegments 0: sendTx: a tx with this nonce is pooled with a fee >= this one" (209 settle_error lines from
     04:23 to 12:56); request_data.bytes_waiting 68,613,923,840 (63.9 GiB) against the 64 GiB stopgap cap,
     refused_storage 466 at 12:30 and 498 at 12:56, so paid AI answered 503 insufficient_storage.
   - Root cause (measured on node 0): the mempool held 32 PostSegments of the gateway key 5wHXuYu1... at nonces
     6527-6558 (the account's confirmed nonce 6527), all ticket 6f120b62... round 1, matrices 32-63, m 1712. On
     the chain that round had started with m 1280 (matrices 0-31 included, standing.round_rows 1280), so every
     one fails at the tip ("round 1 has m 1280, not 1712"). Templates skip a failing tx and nothing evicts it, so
     6527 stayed pooled forever and every later send at 6527 with the same fee was refused. The gateway had resumed
     the half-posted round with `round_take` over today's pending list: the first attempt (9 requests, 1,280 rows)
     was cut after its first 32-tx batch by a gateway restart (02:46 or 03:21 UTC), 3 more requests (429 rows) were
     served meanwhile, and the resume restacked all 12 (1,709 rows, m 1712). Checked: the first 9 pending requests'
     matrix-0 A equals the chain's round-1 matrix-0 A blob byte for byte (getBlob), 8 or 10 do not fit m 1280.
     Not the cause: size or block limits (512-byte txs, max_block_txs 2000), blob availability, fee floor
     (min_fee 1000), ticket state (open, deadline 14744, 628 rows free).
   - Fix (services/inference-gateway): (1) a round is recorded (`posting`) before its first tx and resumed with
     exactly those requests and the chain's m; an older ledger's started round is matched to the oldest-first
     pending prefix whose A equals a posted matrix's A (`round_recovered`), else an error and nothing sent;
     (2) sends sign at the confirmed nonce, read the key's own pooled txs (getMempool + getTx), reuse the same tx,
     replace any other at +10% fee (cap 64 x min_fee), cancel leftovers past the last nonce with a 0 self-transfer;
     (3) a sliding window of 64 in flight (was batches of 32, each waited on); (4) a tx that 5 new blocks skip
     stops that send in minutes instead of 600 s, and a failed ticket backs off (30 s doubling, max 16x) while the
     rest of the pass goes on; (5) settle order: rounds in progress, then most waiting rows under data pressure,
     else oldest. /v1/stats `settle`. CLI `--send-window`, `--stall-blocks`, `--settle-backoff-secs`.
   - Tests: tests/stuck_nonce.rs 5 (chain simulation with the node's mempool replacement rule and templates that
     stop at the first failing tx): the box 0 case end to end (stale m-1712 tx at the confirmed nonce + a leftover:
     recovered with the 2 original requests, stuck tx replaced, leftover cancelled, next round posts), a recorded
     round resumed and its pooled tx reused, a refused ticket stalls in a few blocks while the next ticket posts on
     the stuck nonce in the same pass and the refused one backs off, settle order, replacement fee. Gateway 37/37.
   - ROLLOUT DONE: the orchestrator installed cadre-gateway cb2a6b3b... on box 0 and restarted it at 12:59:19 UTC
     (rollback copies /opt/lode/bin/cadre-gateway.pre-stucknonce, /var/lib/lode/gateway/ledger.json.pre-stucknonce).
     After (measured): the first pass prepared 941712d7 round 0 (1,924 rows, about 5.2 GB of scratch read) and at
     13:02:33 replaced the 32 stuck txs (6527-6558, fee 1100); round_posted 13:08:59; 6f120b62 round 1 recovered
     13:32:29 (9 requests, m 1280) and posted its last 112 segments 13:36:45. By 14:44 UTC: 10 rounds posted
     (rounds_posted 45 -> 55, 16,552 rows), nonce 6527 -> 8066, mempool empty, tx_stalls 0, bytes_waiting
     68,613,923,840 -> 54,405,237,760 (net -8.1 GB/h while 52 new paid requests were served: refused_storage
     only 10 since the restart, all in the first minutes). A cadre-node restart at 13:48:53 (not this row) made 18
     tickets fail getTicket once; they backed off and the pass went on. Throughput is bounded by blocks (about 1.1
     min each, 64 server txs per block by the mempool's nonce window, so at least 3 blocks per 144-segment round)
     plus the round's scratch read; a later speedup is preparing the next round while the current one confirms.
   - 64 GiB stopgap: keep it until bytes_waiting is under 12 GiB after the new binary drains the backlog, then
     restore /root/inference.env.pre-drain (16 GiB cap, 8 GiB disk reserve; the only difference is GATEWAY_EXTRA)
     and restart cadre-gateway once more (safe: rounds in progress are in the ledger now).

39. CHAIN SPEED: WEAK-BLOCK PRE-CONFIRMATIONS + 30 S TARGET WITH UNCLE CREDIT (owner "yeah proceed on speed then
   pls" 2026-10-21; docs/SPEED.md spec + measurements, research/SPEED.md sources; branch speed).
   - Phase 1 (p2p/RPC only): weak blocks = template headers whose Pearl proof meets target << weak_shift (5 at 60 s,
     4 at 30 s: one about every 1.9 s), gossiped as "CWB1" messages on the tips channel (old nodes ignore non-40-byte
     tips, so no handshake change), checked (tx_root over the items, target, timestamp, job, weak PoW; invalid = peer
     blocked); tx status pending / preconfirmed {weak_count, weak_miners, weak_share, first_seen_ms, latest_age_ms,
     confidence} / included / final in getTx, getTxStatus, getWeakBlocks, ws topic weak, submitWeakBlock; a
     preconfirmed tx cannot be replaced by a same-nonce tx; pearl-miner and `agent-node mine` submit weak hits.
     Clients: cadre-api tx_status + GET /v1/tx/chain/{hash}/status, explorer badge with the risk note, `cadre tx
     [--watch]`, extension + pearl-wallet (8b3ac7f15), trading gate services/swarm/trading/src/preconf.ts (NOT hooked
     yet: the two-line hook for the trading row is in TEST-REPORT "Chain speed").
   - Measured: sent -> preconfirmed on another node 1.05 s median, 3.7 s p90 (sent -> included 41 s median); weak
     message 25,025 bytes, check 0.43 ms. Guarantee: evidence of hashrate mining the tx, NOT inclusion (SPEED.md 2.5).
   - Phase 2 (consensus, devnet height 6000): block 6000 migrates target 60 -> 30 s, K 30 -> 60 (30 min kept),
     emission 7 ppm -> 3,500 ppb (config key 45, same 1.003%/day); uncles: up to 2 stale job blocks at depth 1-2,
     1/2 and 1/4 of their subsidy, nephew 1/32, from the pool and the job cap, mature at H + K, tx_root records 0x03
     (137 bytes), body section 0x02; fork choice unchanged (light client unchanged there). Light client: speed schedule
     (set_speed) upgraded on devnet 2026-10-21 slot 513664592, inert until set_speed.
   - Orphans: cross-box 4-node test 0 of 32 at 60 s, 0 of 79 at 30 s, 7 of 1,313 (0.53%) at 3 s, per-peer delay 12 ms
     median; live devnet 4 depth-1 reorgs in 892 blocks (about 0.45%), effective tau 0.27 s, so about 0.9% stale at
     30 s, credited as uncles. Light client: 0.311 SOL/day measured now; 0.58-0.93 SOL/day at 30 s (SPEED.md 7.5).
   - Tests: chain 228 pass (58 suites), crates 74, pearl-cli 119, LiteSVM lightclient 26, header-relay 8, explorer 65,
     cadre-api 98 (+2 env failures, @solana/web3.js missing in that tree, same on main); 8.8 h Phase 2 soak; fresh
     sync of the live devnet from genesis with the staged binary (TEST-REPORT "Chain speed").
   - ROLLOUT (handed to the orchestrator): staged /home/lode/stage-speed/{agent-node 8b4b3f74..., pearl-miner
     9d78dda6...} on boxes 0-2; box 1, 2, 0 BEFORE height 6000 (about 18:40 UTC 2026-10-22), then the light client
     set_speed(6000, 30000, 60) before its tip reaches 6000. Exact commands, checks and rollback: DEVNET.md "Speed
     rollout". If it cannot finish by about 5980, move the height first (never straddle it).

40. JOB QUERY PERF, POOL EVICTION, FAIR SEGMENT ORDER, DOCS DRIFT (2026-10-22, "job query perf" row; owner
   "start working on whatever is left"). Main 64eeaaa (+ docs). All node changes are NODE-LOCAL (no consensus
   change, no activation height): old and new nodes mix, rollback is a binary swap before or after height 6000.
   - Box 0 node CPU, cause (measured read-only on box 0, 2026-10-22): the hot path in the 15:45 UTC perf profile was
     state reads (Store::get, pread, futex hand-offs). RPC by client over 60 s (tcpdump on lo + ss, about 15:55 UTC):
     getJob 400/min explorer, 280/min cadre-settle, 144/min relayer, 100/min cadre-api, 44/min attester; listJobs
     16-47/min from cadre-miner's orphan sweep (`--deliver`, every 60 s it paged `listJobs status=open` 200 at a time
     over about 9,500 open jobs = 48 pages, and every call read 2 records per job for the newest 10,000 jobs: about
     960,000 state reads a minute). A/B on two fresh followers of the live devnet on the test box (same state, height
     5869): that sweep took 20.6 s of node time per pass on the old build, 1.28 s on the new one with the same
     offset paging, and 0.001 s with the new `kind=job` cursor query the miner now sends (one call, 0 jobs:
     every open job is a segment); an auto getBlockTemplate 123-158 ms old, 13-23 ms new. Box 0 node CPU at the time
     (pidstat): 18.97% over 15:56-16:01 UTC and 41.09% over 16:44-16:49 UTC (scripts/devnet-rpc-load.sh), 48.5%
     average since its 15:29 restart; getChainInfo p50 3.3 / 1.8 ms, p90 260 / 410 ms, max 445 / 932 ms in those
     two windows; listJobs 62.4/min (47.2 from pearl-miner), getJob 904/min. See TEST-REPORT "Job query perf" for the full tables.
   - Fix: (1) chain/state read cache: committed values by key (64 MiB, two generations), a read of a key no
     uncommitted block on the snapshot's branch wrote is answered from it, finalize drops exactly the keys the newly
     committed blocks wrote; snapshots record their written keys. (2) chain/rpc job index (jobindex.rs): ids and
     status of every job, records of live jobs and tickets, updated per tip from the keys connected and disconnected
     blocks wrote (Chain::take_touched), so any reorg is handled by re-reading those keys. listJobs answers from it
     (`kind` filter, `after`/`before` cursors, `next_cursor`; offset mode unchanged) and auto templates take their
     candidates from it. getNodeStats: RPC calls and time per method, cache, index, eviction counters. (3) pearl-miner
     sweep: `listJobs {status: open, kind: job}` by cursor (offset fallback on an older node). (4) header-relay settle
     and the relayer read a pending order's job once per chain tip instead of every poll.
   - Mempool eviction: after every tip the node executes its pool against the tip like a template; a tx failing at 3
     consecutive tips is dropped (getTxStatus `dropped` + reason), a failure caused only by an earlier failing tx of the
     same sender does not count, and while a tx fails at the tip a tx at its nonce replaces it at any fee. The
     gateway's own nonce handling (item 39) keeps working; it no longer depends on it.
   - Fair segment order (SPEC 4.5 "Segment order"): auto choice = least-served ticket (blocks mined / allocated) first,
     then oldest; was largest cap_remaining first, which made big rounds always win (ticket ecb7a659, 48 rows, 11 of 144
     after 7 h). On the live state at 5869 the old order spent a simulated hour of 118 blocks on 4 tickets, the new one
     on 13. Measured gap (heights 5491-5850, 6.08 h): 373.6 segments posted an hour against 59.2 blocks an hour (all
     segment blocks); about 118 an hour after the 30 s switch, so about 3x too little capacity at that traffic. No skip
     or refund rule is needed for segments that are never mined (they carry W = P = 0; pay and refund go by rows used;
     their cap stays in the pool); what is lost is miner subsidy and the per-segment GEMM check.
   - Tests (test box ~/jobperf, -j 4): chain workspace 241 pass, 0 fail, 2 ignored (new: state read_cache 3, rpc
     jobindex 5 = index vs full scan across mining, expiry and two reorgs + kind filter + eviction 3, consensus
     fair_order 3, mempool 1); the new agent-node synced the live devnet from genesis (5,869 blocks, cache on) to
     finalized 5839 2a9df3e5... equal to node 1; crates pearl-miner incl. the cursor sweep test pass; header-relay lib
     1 new pass (catchup 2 fail on the box's stale token/target/deploy/lightclient.so, 652,840 B from 10-19; the tests
     need the tree's own build; untouched by this row); relayer 34/34 on the Mac (litesvm bad_alloc on the box, known).
   - STAGED (not running) in /home/lode/stage-jobperf/: agent-node 6a194b23... (boxes 0-2), pearl-miner 9daaf871...
     and header-relay 562cfce6... (box 0). Rollout ONLY after the height 6000 switch is verified, commands in
     DEVNET.md "Job query perf rollout"; then `scripts/devnet-rpc-load.sh 300` on box 0 for the after numbers.
   - Docs drift (cadre-docs c181a66 + this row's follow-up): devnet page (cadre-api, gateway, Solana side, mint,
     upgrade heights), model-registry replay, status, index, tokenomics, limits, jobs, agents, buy-a-job, market (fee
     reaches the pool), x402, data availability, parameters 19-48, run-a-node speed warning; RPC listJobs/getNodeStats/
     sendTx eviction and the segment order.

41. STRATEGIES WITH POST-MORTEMS: PRACTICE TRADES AT MAINNET PRICES, COMMITTED ON CADRE (2026-10-22, "strategies
   with post-mortems" row; owner idea, weft.fi style). docs/STRATEGIES.md is the spec and status.
   UPDATE 2026-10-23: deployed since (deploy/strategies-devnet.json, start 6160); `/v1/strategies` is live.
   - Contract `contracts/strategies` (publish, enter, exit, postmortem; learning rule and forward-only order enforced
     on chain; no new tx kind, no consensus change); 4/4 executor tests with the supply invariant.
   - Runner `services/swarm/strategies` (read-only mainnet client, PumpSwap quotes with the live fee tiers, fills
     priced only after the entry commitment is visible, deterministic analyzer cadre-postmortem-v1, knowledge
     records, three house practice agents tape_lark, floor_wren, still_heron); 14/14 tests.
   - cadre-api `/v1/strategies*` + durable blob store + leaderboard `strategies` field (4 tests); `cadre strategies`.
   - NOT DEPLOYED, NOT RUNNING: the deploy tx and the agents' devnet SOL top-up were refused by this session's
     permission check. Keys exist and hold 10 CADRE each on the devnet (keys/devnet/strategies/). Commands in
     STRATEGIES.md 9; after the deploy, box 0 needs a sync + cadre-api restart for the routes and the deploy file.

42. TOWN AND DMS MOVE TO SOLANA (owner decision 2026-10-22; "town + DMs move to Solana" row; docs/TOWN-ON-SOLANA.md).
   UPDATE 2026-10-23: deployed and live (`/v1/ready` checks.town_solana); all nodes report town_sunset_height 12000.
   New Solana program `town` (token/programs/town; devnet id DAyyoBkS..., localnet 9pgMbpDs...; LiteSVM 13/13):
   profiles, first-come handles, operator links (house flag only from a house operator's link), posts and
   reactions as events, tips, one follow edge per pair, consent, plain and sealed DMs with priced escrow
   (release by reply, decline, reclaim). TIPS AND DM PAY ARE SOL IN LAMPORTS, NOT CADRE (owner 2026-10-22, "for
   now"). Cadre chain gate: DEVNET_TOWN_SUNSET_HEIGHT 12000 (provisional) refuses kinds 14/15/22-24 and 16/18-21;
   ReclaimMessage (17) stays valid. cadre-api: TOWN_PROGRAM turns on a Solana indexer merged with the Cadre history
   at read time (pseudo blocks, one code path for town, DMs and boards; lamports never added to CADRE) and unsigned
   simulated Solana builders (409 with the program's reason). Writers: services/swarm TownNode (house brains,
   desktops, user agents, cadre-agent, trading and market posts), CLI DM send. NOT DEPLOYED: program deploy,
   town-init, operator SOL, box 0 env + restarts, the cadre-web switch (Solana signing, owned by the web row) and
   the node rollout of the gate are commands in TOWN-ON-SOLANA.md section 7. Tests: cadre-api townsol 5/5, real
   validator e2e services/cadre-api/tools/town-sol-e2e.ts.
43. CADRE CONNECT: AN EXISTING AI USES CADRE AT ONCE (owner 2026-10-22, "Cadre Connect" row; docs/CONNECT.md).
   Hosted mode: an agent token (cadre_at_, hashed, shown once; minted with `agent_token: true` at create or
   POST /v1/hosted/agents/{a}/token; rotatable) drives a hosted agent through token-scoped tools that the
   cadre-agents runtime signs inside the spec's limits (services/swarm/agents/src/{tokens,drive,connect-routes}.ts);
   no withdraw/export/limits/pause/delete scope exists. cadre-api: remote MCP /mcp with OAuth 2.1 for claude.ai
   and chatgpt.com (PRM, AS metadata, DCR, CIMD, PKCE, iss, refresh, revoke, consent page) or a static bearer,
   hosted REST /v1/agent/tools, OpenAI aliases at base_url /v1, A2A cards. packages/cadre-sdk (local key,
   limits before signing, escape hatches), packages/cadre-mcp (stdio + local HTTP), packages/cadre-py; none
   published. Devnet runs: examples 01-03, hosted OAuth + MCP e2e (stance filled by the engine), Claude Code
   over http bearer (CONNECT.md "Examples"). DEPLOY (box 0): sync, restart cadre-agents then cadre-api, optional
   caddy mcp.* block. FOLLOW-UP after the town moves to Solana: SDK/py local posts and DMs must use the Solana
   town builders (hosted mode follows through TownNode).

44. AGENT DUMP: A PUBLIC GITHUB RECORD OF WHAT AGENTS THOUGHT, DID AND LEARNED (owner 2026-10-22, "agent dump" row;
   docs/AGENT-DUMP.md). github.com/cadredotag/agent-dump (public) mirrors every agent's posts, hires, deliveries,
   verdicts, strategy versions (with diffs), practice trades and post-mortems, each line with its block, tx hash and
   paid-thought receipt; README explains the two-command check (getTx + gateway receipt). services/agent-dump renders
   it deterministically at the finalized height (plus the Solana finalized slot) from cadre-api; the writer batches
   one commit per 10 min with a state-file cache. scripts/verify-dump.ts re-derives files and checks tx heights.
   State: repo created and ONE push (958d25ac, cutoff 6334); the final code re-derives a fresh dump 379/379 files.
   Next: owner installs cadre-agent-dump.service on box 0 (AGENT-DUMP.md 6); site links profiles to
   agents/<folder> (AGENT-DUMP.md 7); switch to the GitHub App when the toolbelt row ships it (AGENT-DUMP.md 5).

45. SWARMS: A WHOLE SWARM OF AGENTS IN ONE GO (owner 2026-10-22 "allow for swarms", then "less complexity, agents are
    quick executors"; "swarms" row; docs/SWARMS.md). Creator input is goal + size (2..12) + total SOL; two creator
    signatures in all (one signMessage that commits the definition and every member spec, one Solana tx with every
    member's SOL). contracts/swarms LIVE on devnet 654yr1HroSyqBosqFmn2n62MAzjfyrPVMzhjLTDNMQmt (start 6352, admin zero):
    members join only by their own signature, no creator method after create. Runtime: plan/commit routes, members tick at
    once, lead kickoff thought picks the first hire, lead prefers member offers, members deliver swarm hires first, trading
    workers practice distinct styles through PracticeHost. cadre-api /v1/swarms*, /v1/hosted/swarms*, /v1/tx/swarm.
    Measured on devnet: commit to lead kickoff 71.1 s (SOL confirmed at 3.1 s; the rest is the first x402 top-up and the
    thought). Tests: contract 10/10, agents 60/60 incl. swarms 7/7, cadre-api 116/118 (2 need @solana/web3.js installed).
    NEXT (orchestrator): box 0 sync + restart cadre-agents and cadre-api (SWARMS.md 8); sponsor float off until the owner
    sets AGENTS_SWARM_SPONSOR_KEY + AGENTS_SWARM_FLOAT_UNITS; cadre-web must learn agent_spec_swarm in lib/agentspec.ts.
46. DIAG: GATEWAY REQUEST DATA OUTPACES SETTLEMENT; PRACTICE TRADERS NEVER ENTER (2026-10-23, "diag" row).
    - Gateway, measured on box 0: a request's scratch is 2,708,480 bytes per row (rows = prompt + completion tokens;
      144 matrices, A k=2048 int8 = 294,912 B, C int32 = 2,413,568 B, so C is 89%; C is read once, to hash c_hash at
      round time). It is deleted when its round is posted (not at settlement or mining), so the node blob store and
      the miners do not hold gateway disk. Root cause of the backlog: ticket closes ran inside the one settle pass, and
      a close waits for the attester's full glue replay: 480-565 s per ticket of 1,663-1,732 rows (attester log); no
      round posts meanwhile. Live 01:05-01:28 UTC: bytes_waiting 12.60 GB -> 28.36 GB (+40 GB/h) while two replays
      (01:08-01:16, 01:18-01:28) blocked posting. Earlier stalls: blob store full 22:47-01:58 and the stuck nonce
      04:34-12:57 (both fixed by items 37 and 39). Since 16:00 the serve rate averaged 3,683 rows/h (9.98 GB/h).
      Fix on main d379a0d: post and close passes on separate threads (posting skips only the ticket being closed);
      --data-pressure-mb 8192 (early posting at min(cap/2, it)); rows that can no longer post (deadline passed, chain
      closed the ticket) dropped with their data; /v1/stats written_bytes, freed_bytes, pressure_bytes,
      pending_dropped, settle.closing, settle.close_backoff. Tests settle_split 3/3, gateway 38/38 on Linux.
      Not done: zstd of C at rest (measured 0.553 of raw with a 4-byte shuffle on box 0 files, about 45% less) needs a
      new crate in crates/Cargo.lock; attester results are not cached (a gateway restart mid-replay replays again).
    - Practice: conditions do fire (6 decisions 01:17-01:23 UTC on box 0); every decision thought answered the bare
      word "SKIP" (the last word of "TAKE or SKIP?"), and any SKIP vetoed. Fix on main 9950e1d: a SKIP counts only
      when it names a RED_FLAGS signal present in the values (veto_unsupported logged otherwise); watched pools are
      evicted when under minPoolSol or idle 30 min, except a pool with an open trade (the set froze at its first 12).
      Not strictness: a read-only 24 h replay (1-minute vault reads, 9,853 mainnet read calls, 11 pools, 151.7
      pool-hours) had tape_lark's rules hold 6.11% of pool-minutes (147 episodes), floor_wren 0.88% (46), still_heron
      21.18% (154; top10_share not replayable, vol_15m from 1-minute returns / sqrt 3). No calibration change made.
      Tests strategies 15/15, agents 74/74. Box 0 restarts: see the diag report (handed to the orchestrator).

47. HOSTED AGENTS STUCK ON INFERENCE: TOP-UPS SIZED FOR VALUE, QUIET HOLDS, PACED PRACTICE (2026-10-23, "inference top-up
    sizing" row). Box 0 cadre-agents journal: practice_aesw / practice_91mz retried a 402 ("no prepaid balance with
    345 free rows") every 5 s, drv_ejw9ed logged budget_refused every tick, topup_refused "daily SOL cap" with
    price 25,135,765. Live quotes (gateway POST /v1/balance/topup): 1,024 rows 25,135,765, 4,096 rows 25,209,724,
    8,192 rows 25,308,335 lamports; 8,192 is the largest ("multiple of 4 in [4, 8192]", chain max_balance_rows
    8192). The price is almost all a fixed per-order cost, and the runtime bought 1,024-row (or N + 256) tickets,
    so agents reached their daily SOL cap after 1 to 3 top-ups.
    - services/swarm/agents/src/inference.ts (new) + AgentRunner.topup/think/practiceWaitMs: a top-up quotes the
      largest ticket first (AGENTS_MAX_TOPUP_ROWS 8192, cut to max_balance_rows), then AGENTS_TOPUP_ROWS (now 4096),
      then the least that serves the thought, and buys the first whose quote fits the rest of the daily SOL cap and
      the wallet (price + 1,000,000 lamports reserve). Refused = a hold: sol_cap and too_big until 00:00 UTC, wallet
      until the balance changes (re-read once a minute); one inference_hold feed line, nothing quoted or retried
      while held. A thought the day's rows cannot admit returns null without a gateway call, one feed line a day.
      Practice thoughts are paced: rows left today (rows_per_day and what the rest of the SOL cap buys at the last
      quote) spread over the rest of the UTC day at the agent's own average rows per thought; a paced wait is not
      logged and an agent with no open trade skips the scan. Swarm.topup split into quoteTopup + payTopup (house
      behaviour unchanged). Tests agents 85/85 (test/topup.test.ts 10), swarm 45/45, strategies 15/15.
    - ~/cadre-web quick create: quotes the 8,192-row ticket, templates 8,192 rows a day, least funding = own CADRE
      + two tickets + fees (about 0.064 SOL at 25,308,335; swarm members one ticket); the 0.15 SOL default gets an
      80,000,000 cap = three tickets a day.
    - Deploy: box 0 sync main + restart cadre-agents (cadre-swarm optional, its behaviour is unchanged); site
      redeploy for cadre-web. Existing agents keep their signed caps and recover at 00:00 UTC with 8,192-row tickets.

48. CORRECTNESS AUDIT OF THE LAST TWO DAYS (2026-10-23, "chain + contracts correctness audit" row). Reviewed the
    town sunset gate, contracts/strategies, contracts/swarms, contracts/toolbelt and its runtime, the gateway
    settle/close split (d379a0d), and chain/ + crates/ since 2026-10-21 (uncles, weak blocks, blob lifecycle,
    repair, job perf, DMs). Fixed (each with a regression test that fails on the old code):
    - CONSENSUS, chain/consensus-pow/src/chain/uncles.rs + agent-types speed.rs: an uncle could name a job that was
      not mineable at its height (Completed or Expired with cap left, past its deadline, or past max_blocks) and be
      paid 1/2 (or 1/4) plus the 1/32 nephew bonus from the pool. From DEVNET_UNCLE_JOB_RULE_HEIGHT 10,000 an uncle
      must pass executor::check_mineable at U.height (SPEED.md 3.3 rule 7); getParams uncle_job_rule_height. Below
      10,000 nothing changes, so a node replaying from genesis agrees with today's nodes. ROLLOUT REQUIRED: every
      devnet node (boxes 1, 2, 0) on this build before height 10,000 (tip was ~7006 at 00:45 UTC 2026-10-23, 30 s
      blocks: about 25 h). Test chains (other chain ids) apply it from the uncle activation.
    - services/inference-gateway: the close thread snapshotted an "open" ticket, and a request reserved on it while
      SubmitResult and the evaluator ran was served but never posted (stranded on a delivered ticket). The close now
      marks the ticket "closing" in the same ledger step as its re-check. Also served rows are dropped at
      tip == deadline (a PostSegments lands at tip + 1, which the chain refuses above the deadline).
    - services/swarm/agents toolbelt: receipts left `pending` as soon as the commit tx was accepted by the node; a
      commit then dropped or refused left a gap and every later commit failed ("first_seq must equal the agent's
      next_seq") for good; two flushes at once committed the same batch twice and dropped receipts never committed;
      a failed commit after a tool call threw away the paid call's result. Now receipts stay until the contract's
      next_seq passes them (read with `get`), one flush at a time, a resend from next_seq after 10 min, forced
      flushes wait up to 90 s for the last batch, and commit failures are logged and kept pending (TOOLBELT.md 5).
    Checked and fine: the sunset refuses the same kinds in blocks, in sendTx prechecks (tip + 1) and as contract
    native calls; ReclaimMessage stays valid; getParams reports 12000 on all three devnet nodes; nothing differs
    below 12000; strategies/swarms/toolbelt contracts have admin zero on devnet (Migrate/SetContractAdmin refuse);
    toolbelt code 19b68730... rebuilds byte for byte from this tree; strategies a4665db8... and swarms 247ecc72...
    do not (same size, identical strings, function order differs: the wasm build is path/workspace sensitive).
    Rollout of the non-consensus fixes: box 0 cadre-gateway and cadre-agents restart after sync. Details and the
    theoretical findings: TEST-REPORT "Correctness audit 2026-10-23".

49. HOUSE DESKTOPS RESILIENCE: OUT OF CREDIT, SOCIAL MEMORY, DAILY SPEND (2026-10-23, "desktops resilience" row;
    docs/DESKTOPS-AT-SCALE.md 11, services/swarm/desktop/README.md).
    - Box 0: the desktops' OpenRouter account ran out at 2026-10-22 22:24 UTC; 571 tasks failed with model 402 in 5 h
      while all 36 desktops kept running. Now src/credits.ts pauses vision tasks and desktop chain actions on the
      first credit error (one vision_paused line, /status vision_paused + vision_gate), probes every 5 min (free
      /key read, then one tiny request with max_tokens 1600), pauses the desktops after 30 min, resumes on the first
      passing probe (vision_resumed). 402s are no longer retried.
    - Social memory: 1,301 of 1,307 Social memory warnings in 24 h were on one-desktop agents (1 GiB); the
      background activity console and uncapped wallet tabs; 1,000 warnings repaired nothing (alert 80 MB, repair
      60 MB). Console closed when not wanted, 2 tabs on 1 GiB, trim before restart, one threshold, deduped log,
      desk_health line per tick for the after measurement.
    - Spend: hourly spend line, spend_day per UTC day with within_cap, /status spend_days.
    - Tests: desktops 26/26 (test/credits.test.ts 6 on recorded 402 bodies, 5 new in desktops.test.ts).
    - Deploy (handed to the orchestrator): scripts/devnet-desktops.sh code, then restart cadre-desktops. No env
      change needed (defaults in README); every desktop's browser host restarts once (desk_host_updated). Then
      check /status vision_paused and the journal for vision_paused or vision_resumed and desk_health.

50. DEVNET LOOSE ENDS: A REAL PAID HIRE FROM THE SITE, DEPLOY WITHOUT 500s, HEALTH CHECKS FOR TODAY'S OUTAGES (2026-10-23,
    "devnet loose ends" row).
    - Paid hire, end to end from https://cadre.64-95-12-116.sslip.io in headless Chromium (~/cadre-web e2e/hire-paid.mjs;
      throwaway key AGrGGG7j..., Cadre chain txs encoded by the Cadre Wallet's own code): /buy pump buy 0.002 SOL
      (5CRztYDi..., 40,211 CADRE, 6 s), Bridge in 10 CADRE (5F5nzc3T..., unlocked in 120 s), hire #65 of scribe_wren's
      offer 0 with the house judge (5903cc88..., in a block in 57 s), delivered at 6962 (8c96ca8d..., 7 min after the
      hire); the judge never voted (gateway 402 "no prepaid balance with 516 free rows", 3 tries, also hires 63, 66);
      refunded from the hire page after the deadline (94ecc714..., Expired at 7028, 5.3 CADRE back). Hire #68 with the
      buyer as its own evaluator: hire 20e14bae... (11 s), delivered 7ca8f89e... at 7050 (11 min), accepted on the
      hire page db1f08b5... at 7052, Completed (4 CADRE to scribe_wren, 0.8 to the pool).
    - Judge fix (services/attester judge.rs): top up by the largest open ticket's free rows (the gateway reserves a
      request on one ticket; the summed rows_free looked full), --min-rows default 1024 (a judge request is about
      520 rows). Needs the judge on the test box (~/market-build) rebuilt and restarted.
    - /agents/new: no handle lookup until something is typed (cadre-web). Strategy trade pages render post-mortems
      fine; agent-dump lessons.md now carries each post-mortem's lesson text.
    - Site deploy: deploy/site-release.sh builds a release dir beside the live one, carries the old static chunks,
      switches ~/cadre-web-current atomically (DEVNET "Website"); tests scripts/test/site-release.test.sh.
    - Health section 9 (DEVNET "Health check"): desktops 402/failure share/vision paused, OpenRouter balance, gateway
      request data, hosted agents' holds and 402s, practice thoughts; tests scripts/test/devnet-health.test.sh. A
      dry run on box 0 at 03:16 UTC: FAIL desktop_tasks (26 of 26 failed, model 402), FAIL openrouter ($-0.20).
      Still no outward alert channel (owner decision).
    - Found, not fixed: the Privy app does not list https://cadre.64-95-12-116.sslip.io as an allowed origin
      (frame-ancestors refuses auth.privy.io on every page; email login cannot work there): add it in the Privy
      dashboard.

51. STOREFRONT PENDING ORDERS: LIGHT CLIENT STUCK AT 6300, 7 PAID PAYMENTS WITHOUT AN ORDER (2026-10-23, "storefront
    pending orders" row; box 0 read only). At 03:50 UTC: 93 Orders (23 finalized, 6 refunded, 64 pending), 102 x402
    payments in /var/lib/lode/storefront-state.json.
    - What "pending" means: an Order exists only after the x402 payment settled AND buy_job ran, so every Pending
      Order is paid. All 64 have their job on the chain (relayer unlocked them; rows credited): 54 open tickets
      (live prepaid balances, close when used up or expire at posted + 10,000 blocks), 1 delivered, 9 completed
      but not settled because the light client is behind. Nothing pending is a stale quote; quotes create nothing.
    - The light client has been stuck at final 6239 / tip 6299 since 2026-10-22 21:39 UTC: every job posted in
      block 6299 failed begin_verify_job with TxPath (6016), resent forever (failed_txs about 9,970 at 5,000
      lamports each from ops). Cause: header-relay rebuilt tx_root paths from txs + contracts record + coinbase,
      but the chain puts the UNCLE records (SPEED.md 3.3, leaf 0x03) between them; 6299 credits an uncle (66 txs,
      coinbase leaf_index 67). Fix: chain::block_body_of appends uncle records and refuses a body whose rebuilt
      root differs from header_fields.tx_root (no more doomed sends); regression on the real block
      (services/header-relay/tests/block_body.rs + fixtures/devnet-block-6299-uncle.json: node paths equal ours).
      settle (CompleteJob paths) used the same body, so it is fixed too.
    - The explorer showed on_chain_job false for 55 of them: listJobs is a 10,000-job window that segment jobs fill
      in hours. Order jobs outside it are now read by getJob and kept (apps/explorer indexer fillOrderJobs);
      /v1/summary storefront.pending_by_stage.
    - Paid without an Order: 7 payments, 176,900,335 lamports, all legacy records. 6 are crashes of
      storefront-x402 (an uncaught web3.js 429 exits node right after the payment was taken: 10-20 18:03, 10-21
      16:41, 16:42, 16:43, 17:00, 17:01 UTC), 1 is a refund that failed on a 429 (10-21 16:06). Six OrderDrafts
      (4,109,720 lamports each) are left open by them. Fix: main.ts survives the 429 throw/rejection; records are
      v 2 with placement steps (draft, opened, buy_sent, cancelled); reconcile() every 60 s refunds v 2 payments
      without an Order once no buy_job can land, retries failed refunds, and holds for review when a buy may have
      landed; GET /payments/unserved; src/reconcile.ts for the legacy ones (list, --check, --refund, --cancel-drafts,
      refuses while the service runs). Tests: storefront-x402 20, header-relay 1 + 3 + 8 catchup, explorer 68.
    - Deploy and the refund steps: DEVNET.md "Storefront pending orders (2026-10-23)".

52. JUDGE TOP-UP 400 AND THE STOREFRONT DRAFTS (2026-10-23, "judge top-up + storefront drafts" row).
    - Cause: cadre-storefront was stopped 04:39:20-04:59:48 UTC to run reconcile.ts --cancel-drafts (the tool did its
      refunds, then hung). Meanwhile the gateway's top-up called the storefront, got "connection refused", and its
      catch-all turned that into 400 invalid_request_error, so the judge logged "top-up quote: status code 400".
      Nothing in the judge's request was wrong (ureq sends the same POST {"rows":2048} as curl; it got 402 again from
      05:00). Hire 73 then used its 3 tries on gateway 402s and the judge skipped it from then on; it expired at
      7180 unjudged (buyer refunds from the hire page).
    - Fixes on main: gateway topup and per-request x402 answer 503 upstream_unavailable + Retry-After 30 when the
      storefront or the blob store does not answer (tests/upstream_down.rs). Judge (judge.rs): failures before a paid
      answer (gateway 402/408/429/5xx or transport, failed top-up, node reads, blobs not served yet) are free retries
      with a 20 s doubling backoff (10 min cap) and do not use the 3 tries, which now count only answers that cost a
      thought; the top-up error logs the gateway's body.
    - Test box judge rebuilt (~/market-build, judge.rs from 3143f7e) and restarted 05:42 UTC, pid 1132393: topped up
      2048 rows; verdicts REJECT 77 (107dc1b6...), REJECT 78 (414e59be...), ACCEPT 79 (c5cd8027...).
    - Not fixed: hires 76 and 81 got "no VERDICT line" 3 times each. Decoding is greedy, so the 3 tries give the same
      answer and cost 3 thoughts; the 1B model's format is the problem (judge prompt / parse_answer).
    - storefront-x402: bounded RPC (30 s requests, polled confirmation <= 90 s, no websocket), reconcile.ts always
      exits (watchdog), --cancel-drafts runs with the service up and skips drafts an unserved payment names, and the
      service sweeps orphaned drafts itself every 10 min (test/rpc-bounds.test.ts, 2 new tests in service.test.ts;
      29 pass). The 6 drafts are still open and orphaned; deploy = DEVNET "Storefront draft sweep".

53. PAID INFERENCE THROUGHPUT ON THE DEVNET CPU WORKER (2026-10-23, "tput" row; box 0 read only). docs/SPEED.md 8,
    docs/measurements/tput-2026-10-23/. Box 0: worker busy only 28%, 1.59 s per step at 4.5 rows; gateway 13-17k
    rows/h, no gateway queueing. The time was (a) OpenMP spinning on an oversubscribed 4-core box (attester replay
    145%, miner 118%, node 73%): a 32-row step 0.85 s -> 10.4 s under a stand-in load, 1.8 s with
    OMP_WAIT_POLICY=PASSIVE; (b) every decode row padded to a 32-row GEMM (79% of a decode step); (c) prefill glue
    (binary64 lm_head scale-out for rows nobody reads, two int64 divisions per SiLU element); (d) live rows 2048 +
    FIFO prefill: short thoughts waited for a report's whole prefill.
    - tools/cadre (bit-exact: golden.py equal to the old code on all A/C files, tokens and logits; test_batch 28/28
      with 7 Rust replays; test_replay 19/19): CPU int_mm without padding, lm_head logits for last rows only,
      in-place scale-out, one SiLU division, binary32 CPU KV cache (exact, half the memory), --prefill-order short
      (default, 8-step starvation bound), /health prompt_rows + prefix_rows (measures what a prefix cache would
      save; nothing cached), profile_step.py, bench_mix.py, golden.py. Unit: step rows 32, live rows 4096,
      OMP passive; attester Nice=10.
    - Measured on the same trace (box 0-like load, 4 CPUs, 2 threads): thoughts' first token 443.6 s -> 55.0 s mean,
      rows/s 5.3 -> 15.2; idle 106.6 s -> 24.4 s. Single decode step 0.498 -> 0.159 s; 64-row prefill 25 -> 39 rows/s.
      Peak RSS 3.12 GB (old, live 2048) vs 3.01 GB (new, live 4096).
    - Compression of C (item 46): zstd 1 + shuffle 0.569 of raw, about 240 MB/s per core; not implemented (disk
      drains since d379a0d, CPU is box 0's bottleneck, new C-library crate); numbers in SPEED.md 8.4.
    - DEPLOY (not done by this row, box 0 only, no build, no gateway change): see SPEED.md 8 and the row's report:
      sync main, worker drop-in (CADRE_MAX_STEP_ROWS=32 CADRE_MAX_LIVE_ROWS=4096 CADRE_PREFILL_ORDER=short
      OMP_WAIT_POLICY=PASSIVE), restart cadre-worker when /health active is 0; attester drop-in Nice=10, restart
      cadre-attester when /v1/stats settle.closing is null.
54. JUDGE VERDICTS AND PRACTICE LESSONS WITH THE 1B MODEL (2026-10-23, "judge verdicts with the 1B model" row).
    - What the model emitted: hires 76 and 81 got `VERDICT: VERDICT: REJECT\nREASON: ...` (label doubled) under
      prompt v1; the old parser saw no verdict word after the first `VERDICT:` and paid 3 identical thoughts each.
      Proven by replay: `pearl-evaluator --judge-probe` (new, never votes) gave answers whose sha256 equals the
      text_sha256 of the original receipts (req_3ede2c2b..., req_c2779eca..., req_8c9bc163..., req_72f26401...,
      req_09d1b8f4...). The offer spec blobs were already pruned, so the judge saw no promise at all.
    - Judge (services/attester/src/judge.rs, prompt cadre-judge-prompt-v2): `/v1/completions` with the chat
      template written out and the assistant turn begun with `VERDICT:`; first word must be ACCEPT(ED) or
      REJECT(ED) (markdown, case, a doubled label tolerated; `NOT ACCEPT`, `ACCEPTABLE` refused). One paid
      thought per hire, then ABSTAIN (no vote, buyer refunded at the deadline, MARKET 5); a failed verdict tx is
      resent from the kept answer. A shorter one-word retry form was measured and dropped: ACCEPT on all 11 it
      answered, junk included. New mechanical rejects without a thought: refusal, copy of the request, link,
      delivery that writes a verdict or tells the judge to ignore the rubric. Missing spec: the promise line is
      left out (a placeholder there was quoted back by the model). Gateway 400 "worker Connection refused" is a
      free retry (the gateway reports a down worker as 400; seen 07:4x UTC; gateway-side 503 is still to do).
    - Measured on 12 recorded cases (6 real hires, 6 synthetic bad deliveries): verdict word 12/12, matches the
      reviewer's reading 8/12 by the model alone, 10/12 with the mechanical checks; still wrong: hire 78 (five
      fine names rejected) and "two names for five" accepted. Fixtures: services/attester/tests/fixtures/
      judge-answers.json; tests 10 judge unit (attester 25 total).
    - Lessons (services/swarm/strategies thoughts.ts, agent.ts): the lesson now starts with the analyzer's facts
      written by code (`lessonFacts`), the model gives one answer-first `FIX:`/`KEEP:` line for the analyzer's
      bucket, and `lessonContradiction` drops it if it contradicts the facts (no-fill or flat called a gain or
      loss, win/loss swapped, wrong SOL amount, a bucket not found, a reply to "your review"). On the 26 real
      devnet post-mortems the guard drops 8 old lessons (trade 9, a no-fill called "a significant loss"); the
      new prompt on 6 of them gave 6 answer-first lines, all passing the guard. Blob adds lesson_facts,
      lesson_model, lesson_guard. Fixtures: test/fixtures/lessons.json; strategies 18/18, agents practice 12/12.
    - Deploy: test box judge rebuilt + restarted (see the row); box 0: sync main, restart cadre-practice and
      cadre-agents (DEVNET steps in the row's report).
55. BOX 0 NODE BLOB STORE FULL AGAIN; PAID INFERENCE REFUSED (2026-10-23, "node blob store full" row; box 0 read only).
    - Measured 10:07-10:27 UTC: gateway /v1/stats request_data node_blob_headroom_bytes 685,020,117 then 630,431,438,
      refused_node_storage 372 then 468 (rising). Node getBlobStats: total 37,314,979,883 of max_total_bytes
      40,000,000,000 (node.json `blobs`, node-local), rpc_put_limit 38 GB (rpc_reserve 2 GB), segment_a 35.43 GB in
      27,120 blobs, ALL kept, 0 prunable; model 1.78 GB pinned. Disk 145 GB, 77 used, 68 free; node-blobs dir 35 GB.
    - What limits it: the node-local quota only (not disk, not a consensus param). What frees A: SPEC 4.5 retention,
      node prune pass (getBlobStats last_prune_height follows the tip). keep_replay is true on box 0, so a live ticket's
      segment A is kept until the ticket's deadline + W (720 blocks); 73 of the 100 tickets in the gateway journal are
      Open, deadlines 14,737 to 17,582 at tip 7,824 (about 6,900 to 9,760 blocks ahead, 2.5 to 3.5 days at the measured
      30.8 s a block, 7783 -> 7824 in 21 min). So almost nothing is prunable until those tickets close.
    - Why now: the 2026-10-22 sizing (SPEC 4.5) assumed 20 live balances and about 2,200 rows an hour. Posted rows from
      the gateway journal: 126,348 rows in 222 rounds over 12 h (about 10,500 rows an hour, about 2.4 GB of A an hour at
      231,424 bytes per row), 172,384 rows over 24 h; the last hour 4,724 rows (throttled by the refusals). Item 47
      (top-ups buy the largest ticket first, up to 8192 rows) and item 53 (worker throughput) raised both the number of
      live tickets and the rows. Not a code bug: retention works as specified; the live-ticket replay hold no longer fits
      the 40 GB quota at this traffic.
    - Gateway refusal: node_storage_refusal compares (waiting rows + request rows) x a_bytes_per_row (144 x 2048 =
      294,912, an upper bound; real blobs are 231,424 per row plus padding to the round period) with the headroom, so at
      5.0 GB waiting it refuses nearly every paid request while rounds keep posting.
    - Health: scripts/devnet-health.sh 9c now emits `node_blobs` (WARN under MIN_NODE_BLOB_HEADROOM_GB, default 2, or
      when refused_node_storage rose since the last run, or when the gateway cannot read getBlobStats).
    - Proposed fixes (NOT applied; headroom was above the 300 MB apply threshold): see the row's report. Option A: raise
      box 0 `blobs.max_total_bytes` 40e9 -> 60e9 (backup node.json, restart cadre-node only); about 19.7 GB more RPC room (5% reserve), about 8 h at the
      12 h rate. Option B (policy decision): box 0 `keep_replay` false like boxes 1-2, frees the A of every live ticket
      outside the mining rule; costs fraud-proof localization data for live tickets (honest replays read no A).
    - FIXED in the gateway (2026-10-23, "gateway node blob estimate" row; not deployed): node_storage_refusal now sizes
      the A the waiting rounds will put with the blob encoder's layout, `12 + m x k` per DISTINCT A blob (matrices with
      byte-identical A share one content-addressed blob; learned from a request's a/*.i8 files or a posted round's
      segments, 113 of 144 on box 0), each round padded to the row period and packed like round_take; a round half put
      by the posting thread counts only the blobs still to put (that was a double count: its rows stayed pending after
      their blobs were already in the node's total). Box 0 backlog at 10:40 UTC: old 706,904,064 bytes, new 555,421,668.
      Of 467 refusals logged by gateway clients 07:54-10:35 UTC, 453 would have been admitted (old need x 231,424 /
      294,912 + 10 MB of padding still under the headroom). /v1/stats request_data adds node_blob_need_bytes,
      a_bytes_per_row, a_blobs_per_round. Tests: tests/node_blobs.rs (estimate == bytes put, half-posted round,
      box 0 recorded backlog).

56. SECOND QA PASS OF THE DEVNET PRODUCT (2026-10-23, "qa2" row; headless Chromium, throwaway keys; box 0 read only).
    Found and fixed (each with a test that fails on the old code), NOT DEPLOYED:
    - LIGHT CLIENT STALLED AT 7497 since 07:50 UTC (all three relay lanes idle, lag growing, settle and storefront
      orders waiting): two blocks at 7497 with equal work; the light client kept b3af..., the node 5232..., and
      7498-7502 mine jobs posted in 5232. begin_verify_job wanted the posting block final or canonical and reorg wants
      strictly more work, so the node's branch could never verify. token/programs/lightclient: a Verified posting block
      on the job block's own branch is accepted (its parent, or through at most 16 Verified ancestors passed as
      remaining accounts, each linked by prev hash); client::begin_verify_job_via; header-relay bind_for passes them.
      LiteSVM equal_work_fork_at_the_tip_does_not_stall_job_blocks; lightclient suites and header-relay catchup green.
      DEPLOY ORDER: upgrade the devnet lightclient program 47mavKip... first (devnet deployer), then header-relay on
      boxes 0, 1, 2. Once 7498 verifies, the relay's existing reorg step moves the tip to the heavier branch.
    - PROFILE BLOBS PRUNED: 16 of 18 hosted agents showed profile_blob false (no verified spec, no practice label on
      the profile Trading tab, no persona). A node pins only the profile blobs its own state references (SPEC 4.6);
      SetProfile is a Solana tx since the town moved, so the node prunes the blob after its grace. The runtime now
      puts the same bytes back (same URI) when the node lacks them, checked every 10 min (agent.ts keepProfileBlob).
      Deploy: box 0 sync + restart cadre-agents. Lasting fix (open): pin Solana-town profile blobs (cadre-api's town
      index could hold them in its durable blob store, or the node could take a pin list).
    - Trading board "User agents with real swaps" listed practice agents: a buy whose exact CADRE is bridged out
      later (the runtime's fee self-funding) is no fill (fee_buys); an agent is listed with real swaps on or real fills
      on record (cadre-api trading.ts). Deploy: cadre-api restart.
    - Swarm plans took 1 lamport per member through the API; a QA swarm of 0.03 SOL per member is held for good
      (fee CADRE + a 0.0251 SOL ticket pass its cap and wallet). The plan now refuses a share below fee CADRE + one
      ticket at the newest quote + reserve + fees. Deploy: cadre-agents restart.
    - Site (cadre-web 1674685, fcdf647): town and DM signing waits for the town's chain instead of "Still loading";
      the follow button waits for the follow state (it said Follow while loading and sent a refused follow); the
      focus post refreshes; long notes wrap on a phone. Deploy: site release.
    Passed: every sidebar page desktop and phone, dark and light (console, overflow, em dash, monospace, raw JSON);
    practice agent live in 2.3 min, first practice entry at 37 min; toolbelt web researcher 8/8; paid hire #86 with
    the rebuilt judge (ACCEPT, completed in 11 blocks); Connect OAuth + MCP (22 tools) with cadre_trade refused for a
    practice agent; agent dump fresh with lessons. Health WARN/FAIL read in the row's report.

57. AUDIT FOLLOW-UPS: UNCLE REPLAY, DELETE ON FINAL, DEPARTED LEAD, ATTESTER HEALTH, LIGHT CLIENT LAG (2026-10-23,
    "audit follow-ups" row; TEST-REPORT "Audit follow-ups 2026-10-23").
    - CONSENSUS, uncle replay closed by pow_digest (SPEED.md 3.3 rule 8): a Pearl proof opens one tile under the
      target, so a nonce whose GEMM has two such tiles has two valid proofs for one header; the hash covers
      proof_hash, rule 4 compared hashes only. From DEVNET_UNCLE_DIGEST_RULE_HEIGHT 13,000 an uncle whose
      pow_digest equals the branch block's at its height, another uncle's in the block, or an uncle's of a branch
      block above its height is refused; templates offer one uncle per pow_digest; getParams
      uncle_digest_rule_height. Clean: every header field except proof_hash is inside pow_digest, so equal digests
      are the same block; a real sibling always differs. Test consensus-pow tests/uncles.rs
      a_second_proof_of_a_block_is_not_paid_as_an_uncle (fails on the old code: the template carries two).
      ROLLOUT REQUIRED: every devnet node (boxes 1, 2, then 0) on this build before height 13,000 (tip 8,505 at
      16:08 UTC 2026-10-23, 31.1 s average blocks: about 38 h). All three nodes already report
      uncle_job_rule_height 10000 (HANDOFF 48 rolled out), so this is a rolling upgrade of its own.
    - Gateway (not consensus): request data kept until the round's last PostSegments is final, then deleted; a
      round a reorg dropped (last tx neither in a block nor pooled on 3 passes) is posted again from it. Measured:
      final = 60 blocks = about 31 min (6100-7000: 31.1 s per block), at 9.98 GB/h about 5.2 GB more on disk
      (5.9 GB at 35.3 s blocks); counted in the 64 GB cap, not in the 8 GB pressure line. --delete-on-post rolls
      back. Also send_many now accepts a tx already "final" (it waited for "included" only). Tests retention 6/6,
      gateway 48 passed after the merge (2 ignored on macOS).
    - Swarms: a departed lead's proposals stay acceptable on the deployed contract (admin zero). The hosted runtime
      declines them for invitees it hosts, cadre-api builds no accept for them (409 stale_proposal) and labels
      standing; a future contract version closes them on leave / hand_lead (SWARMS.md 3). Tests agents 101/101,
      swarm 45/45, cadre-api 129 passed (10 skipped), typecheck clean (after the merge).
    - Attester: the FAIL "cli:local_attester timed out reading response" (box 0, 07:33-08:47 UTC) was real
      blocking, not a short timeout: one tiny_http loop served /health and /result, so a health check waited for the
      whole glue replay (journal: five queued GET /health answered in the second POST /result finished, 07:51:46).
      Now /health is answered on the accept thread, /result and /share on --workers threads (default 1: replays stay
      one at a time, as the gateway's close thread sends them), /health carries busy {running, queued, done,
      oldest_running_s}, and the CLI line shows a replay in progress. Settlement did not stall beyond that (only the
      gateway's close thread calls /result, one ticket at a time). Tests attester 26 (serve.rs new), cadre-cli 121.
    - Light client lag: not the Solana rate limit. Boxes 1 and 2 already run the header relay on the Helius
      devnet RPC (/etc/lode/solana-rpc-url on all three boxes since 2026-10-21 21:26 UTC; the relay processes'
      command lines name devnet.helius-rpc.com; zero 429s in today's journals). The light client sat at 7,497 from
      07:53 to 16:03 UTC (relay errors 6027 / 6023 / Custom(0) around a node reorg at 7,496-7,498, then idle) and
      moved again after the 16:02-16:04 restarts ("reorg to 1fbdad8f... at 7498"): 7,498 -> 7,543 in 8.2 min
      (5.5 headers/min) against 1.7 blocks/min, lag ~970. The stall is HANDOFF 56's equal-work fork at 7,497
      (qa2 row: lightclient + header-relay fix, which the 16:02-16:04 restarts match). Box 0 also runs a manually started cadre-header-relay (unit disabled, no with-rpc, public
      RPC, ops payer, no --lane) that relays the same heights as the two lanes (failed_txs 202). Steps in the
      TEST-REPORT section; no box config changed by this row.
    - Rollout (owner; nothing deployed by this row): node build to boxes 1, 2, 0 before 13,000 (one at a time, check
      getParams uncle_digest_rule_height 13000 and finalized-hash agreement after each); box 0 cadre-gateway (no
      flag change) and cadre-attester (no flag change) restarts after the sync; cadre-agents and cadre-api restarts
      for the swarms change; cadre CLI binary for the health line.

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
