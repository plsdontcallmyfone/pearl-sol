# Cadre public devnet

The public devnet of Cadre (ticker CADRE): the agent chain on three of our own boxes, the token
side on Solana devnet. Everything here is devnet only: no mainnet, no pump.fun mainnet launch.
Owner decisions (2026-10-18): public devnet first; 3 new BitLaunch boxes, one node each; the
docs/SPEC.md chain parameters and buyback defaults unchanged; no domain, sslip.io names with https.

Status (2026-10-20): LIVE end to end, e2e run 2 7/7 PASS; website and paid AI (Llama-3.2-1B-Instruct,
CPU) live on box 0 since 2026-10-20, one real paid flow settled by proof ("Inference" below). Agent chain on 3 nodes (open p2p, miners on every
box), upgraded in place (rolling, chain kept finalizing) to commit fe8e04a, which has RejectResult
(tx kind 13), RegisterModel / PostSegments and the token-side changes. Solana devnet programs deployed
from the same commit, initialized, light client following, proof modes on, relayer, header-relay,
storefront-x402, crank, explorer and cadre-api running on box 0. e2e results: "End to end results" below.

## Endpoints

| what | URL |
|---|---|
| agent-chain node 0 JSON-RPC (docs/RPC.md) | https://rpc.64-95-12-116.sslip.io (WebSocket: `/ws`) |
| node 1 | https://rpc.64-95-13-213.sslip.io |
| node 2 | https://rpc.64-95-12-22.sslip.io |
| p2p seeds (open membership, `key@host:port`) | `64.95.12.116:10640`, `64.95.13.213:10640`, `64.95.12.22:10640` (keys: `agent-node pubkey`, or `getPeers`) |
| explorer (docs/EXPLORER.md, JSON under `/api`) | https://explorer.64-95-12-116.sslip.io |
| storefront x402 (buy AI jobs in SOL) | https://storefront.64-95-12-116.sslip.io (`/health`, `/quote`) |
| relayer health (GET only) | https://relayer.64-95-12-116.sslip.io/health |
| mint metadata (pump.fun uri) | https://explorer.64-95-12-116.sslip.io/cadre-token.json |
| Solana RPC | https://api.devnet.solana.com (public; rate limited) |
| cadre-api (public site API, docs/API.md) | https://api.64-95-12-116.sslip.io/v1 (`/v1/ready`, `/v1/summary`, ...): `cadre-api.service` on box 0, :10685 behind caddy; gateway and prepaid ON since 2026-10-20 06:17 UTC (`/v1/openai/*`, `/v1/inference/*`), pool off |
| Cadre website (~/cadre-web) | https://cadre.64-95-12-116.sslip.io (`cadre-web.service` on box 0, :10710 behind caddy); miner installer at https://cadre.64-95-12-116.sslip.io/install-miner.sh (devnet defaults) |
| house desktops (ECOSYSTEM 2.10) | https://cadre.64-95-12-116.sslip.io/live (grid) and /live/<handle>; API `/v1/desktops`; `cadre-desktops.service` on box 0, 127.0.0.1:10720 (see "House desktops") |
| paid AI (inference gateway, SPEC 5.4) | through cadre-api: base URL https://api.64-95-12-116.sslip.io/v1/openai (OpenAI-compatible), model `llama-3.2-1b-instruct-w7a7`; gateway :10664 and worker :10665 on box 0 (127.0.0.1 only) |

CLI: `pearl` now defaults to the devnet (`--network devnet`); `--network localnet` (or
`PEARL_NETWORK=localnet`) gives the old 127.0.0.1 endpoints and localnet program ids.
Wallet: the "Cadre Devnet" network preset is the default in ~/pearl-wallet (dev).

## Chain (genesis `deploy/genesis.json`)

| | |
|---|---|
| chain_id | 1128350802 (0x43414452, "CADR") |
| genesis hash | 51c86b676bfd1b9f0877e6994cff95ab92cc1f2c784b39fc943b8b0d4d52028a |
| params | `{}` = the SPEC / `Config::defaults`: 60 s blocks, K 30, emission 7 ppm, alpha 0.8, WTEMA half life 1 h, min_fee 1000, min_protocol_fee 0 |
| genesis target | 0x1e0f1b3f (from a measured 6 threads x 3,085 nonces/s; too easy by about 1000x: the first ~1,000 blocks came fast while WTEMA hardened it, settling toward 60 s within about an hour) |
| admin (AdminConfig) | ops `Bpxwemu6qrKfkmCUJ9y6daFdN1o5iiZvY9KopXWFT9Zb` |
| attester (chain + bridge signer + storefront attester), 1 of 1 | `9U5wdELZRod6PGxczjyLn91meTHuzVLAoEnFywczC5VD` |
| miner payouts | box 0 `GDcEUhU4fXRziN3YQyrwyoZJhsGhSRytJooX7kr7hC4s`, box 1 `4pDphQUNYsybNnXH4nkHdHMS6udinkjVuFrf6iRZpU8b`, box 2 `AHYHqULAUjJzvBkNsRYFpksrbjL1qqurZcgMWq9mHdAH` |
| scheduled upgrade | height 4200: RegisterModel takes up to 1024 matrices (was 384; SPEC 4.5, HANDOFF item 21). Every node, ours or outside, must run main from 2026-10-20 or later before 4200; an older node forks off at the first block carrying a RegisterModel over 384 matrices |
| binaries | agent-node 6661bb6b... ("fix3", branch follow-stall 7c641c8 = main of 17:55 UTC + the blob flow fix, HANDOFF item 24; built on the test box, cadre-node restarted box 1, 2, 0 about 19:05 UTC 2026-10-21; previous binary kept as /opt/lode/bin/agent-node.pre-fix3) and pearl-miner fe576acc... on all three boxes. Before: agent-node 7ce3d9a1... built on box 0 from main 816c941 (2026-10-21 rolling upgrade: RegisterModel 1024 matrices from height 4200, social kinds from 3260); other services as before. Earlier: built on box 0 from commit fe8e04a (agent-node sha256 3a80b545...91f277d0, pearl-miner 356437dc...3b52ee0); was 424c2c1 until the 2026-10-19 rolling upgrade |

## Solana devnet programs (NEW keypairs, `--features devnet` builds)

| program | id | .so (sha256) |
|---|---|---|
| reserve | 8m1RBPiiscHWpXRDj4NevSFMN68tbgCC8f4hu6N4hVzi | 386,216 B, 13f78f26...613570c4 |
| bridge | 3S226LmVvsoA4QrEpwytiuLxREJrQHS7A1UkeDsJGFoN | 398,248 B, 1f6abc9c...2f7970e0 |
| storefront | 9XjvGk2xJdByPL6NmUCQg7RdBLyeDjWjikZBnwuWvAKu | 624,544 B, ad947c9a...2e717088 |
| lightclient | 47mavKipT2URcdj9qp2Stv6H5cM41cmzDXiTEcmDie4m | 665,536 B, 2017132c...657c0b (upgraded 2026-10-21, begin_verify_job; was 652,840 B, e8786d59...5d58aef) |

Deployed 2026-10-19 from commit fe8e04a (build on box 0), max-len 1.1x the .so size (programdata
424,837 / 438,072 / 686,998 / 718,124 B).
Upgrade authority: deployer `Fvqtw8jwj9Y97hMC5mu2VYRhAAi5z8GXHhp76X4qXRaP` (key on the Mac only).
pump.fun: the real programs are live on Solana devnet (pump 6EF8..., PumpSwap pAMM..., fees
pfee..., Global and fee_config accounts initialized, transactions every few seconds), so the
devnet CADRE mint is a real pump.fun create_v2 launch (name Cadre, symbol CADRE, 1 SOL dev buy
into the reserve). No fake pump.fun.

How the ids are switched: each program has `#[cfg(feature = "devnet")] declare_id!(...)`
(storefront also its DEFAULT_RESERVE_PROGRAM); header-relay `--features devnet`; the TS services and
the explorer read `BRIDGE_PROGRAM`, `STOREFRONT_PROGRAM`, `RESERVE_PROGRAM`, `LIGHTCLIENT_PROGRAM`
from the environment (`/etc/lode/services.env`); IDLs with the devnet addresses: deploy/devnet-idl/.

### What is on Solana devnet

| | |
|---|---|
| CADRE mint (Token-2022, pump.fun create_v2, name Cadre, symbol CADRE) | BwJqbsJUXshRj9ABxXs4RZ838kZrSqw8TT4CKMNfEu5e |
| pump.fun bonding curve (graduated) / PumpSwap canonical pool | AGxUczNjaySraHYotmW2vS6d2NcNQXfWq8cdwPzQ2piV / `canonicalPumpPoolPda(mint)` |
| reserve Config (layout 1) | eYzQHvueBMcSCxiU8mM75Kxb6PLAGEn3DttvBFQNvd1, admin ops, creator PDA 9LcVub7iBYpXwidnqEDdrV3pfZVkMmdac3ooN6f4monD |
| bridge Config | AfRovjWYspv13E9rtT8nJj3T9xirSdmZPj2c7zxbXtUM (attester 1 of 1, release mode proof only) |
| storefront | buyback vault 9D3hqn9D2TsT4pS7p7J5sAHNB9QBHVJnPSKX8d7kJbQ6; finalize mode proof only |
| chain mining pool (frozen in the reserve at the first fund) | 6LpEAcFWjE1EswQ4zNdAkgXRjPS1dm7KDhsewnXSRu6v |
| light client | initialized at chain height 1655 (header-relay init on box 0); relayed from box 1 since 2026-10-19 23:08 UTC |

pump.fun on Solana devnet is not mainnet-shaped: its Global has initial virtual reserves of 0.3 SOL
(mainnet 30 SOL), so the 1 SOL dev buy bought the whole curve (822.37M CADRE into the reserve) and
pump's devnet migration bot ran MigrateV2 two seconds later. CADRE on devnet trades on PumpSwap; the
storefront quotes `venue: pumpswap` (buy_job_amm), the crank uses crank_buyback_amm, and
services/crank/src/trade.ts buys and sells on the PumpSwap pool once the curve is complete. Because the
reserve already holds far more than the 100M `bb_reserve_cap`, every storefront buyback on devnet burns
(`nextBuy: burn (reserve at cap)`); on mainnet the curve does not graduate on the dev buy.

Token-side settings after init (fresh initialize, so `migrate_config` and `sf-set --reserveCap` were not
needed: the Config already reads `layout 1`): fund limits 10M CADRE per call, 3600 s between calls, 20M
per 24 h, not paused (all PROVISIONAL); storefront buyback 1000 bps to the reserve, 1 SOL per call, min buy
0.001 SOL, 600 s, reserve cap 100M (PROVISIONAL). First fund_mining: 10M CADRE (deposit nonce 2).

SOL spent (devnet, 2026-10-19): deployer 89.9999 -> 67.9612 SOL: 11.5387 programdata rent + fees for the
4 deploys (buffers refunded), 10 SOL to ops, 0.5 to the storefront wallet. Ops: 10 -> 8.84 after init
(1 SOL dev buy, two 0.02 SOL gas buys, rents for the configs, accumulators, light-client accounts).

To redo from scratch (new keypairs would mean new ids everywhere; normally only upgrades are needed):

    scripts/devnet-solana.sh balances
    scripts/devnet-solana.sh deploy          # 4 programs, max-len 1.1x (or `deploy storefront` to upgrade one)
    scripts/devnet-solana.sh fund 10         # deployer -> ops 10 SOL, storefront wallet 0.5
    scripts/devnet-solana.sh init            # idempotent; keeps a local record in .devnet-e2e/init-done.json
    scripts/devnet-e2e.sh                    # end to end from the Mac

## Boxes (BitLaunch, Dallas dal1, Ubuntu 24.04 LTS)

| box | name | id | IPv4 | size | runs | $/hr | $/month (cap) |
|---|---|---|---|---|---|---|---|
| 0 | lode-devnet-0 | 6ad580a0b64e983f5dbc913c | 64.95.12.116 | nibble-8192: 4 vCPU, 8 GB, 150 GB | build box; node 0, miner (2 threads, delivers), attester, relayer, cadre-settle, storefront-x402, crank, explorer, cadre-api, cadre-web, inference worker (2 threads) + gateway, caddy | 0.156 | 104 |
| 1 | lode-devnet-1 | 6ad585c5b64e983f5dbc916a | 64.95.13.213 | nibble-2048: 2 vCPU, 2 GB, 50 GB | node 1, miner (2 threads), caddy | 0.039 | 26 |
| 2 | lode-devnet-2 | 6ad585e3b64e983f5dbc916d | 64.95.12.22 | nibble-2048: 2 vCPU, 2 GB, 50 GB | node 2, miner (2 threads), caddy | 0.039 | 26 |

Total **$156/month** (BitLaunch caps each box at its monthly price; $0.234/hr until the cap).
Account balance before buying: $196.67; the account limit is 5 servers (now 5 with pearl-sol-test
and instance-backend). Box names say "lode" because they were bought before the Cadre rename;
internal user `lode` and paths `/etc/lode`, `/opt/lode`, `/var/lib/lode` likewise.

Sizing, measured on box 0 (one node + pearl-miner, 2 threads, 2026-10-19): agent-node RSS 458 MB
at height ~200, under 5% of a core; pearl-miner 5 MB RSS, one core per thread, 3,085 nonces/s per
thread. A 1 GB / 1 vCPU box would leave under 300 MB and no core for the node; nibble-2048 is the
smallest with a core for each. Box 0 is 8 GB because it builds (chain build peak a few GB, anchor
build) and runs all the services. Each box has a 4 GB swapfile. Watch node memory: the node keeps
blocks in memory on restart (docs/HANDOFF.md M1 node notes), about 25 KB per fallback block.

Kernel: all three boxes rebooted into 6.8.0-147-generic (with libc6) on 2026-10-20 ~15:25-15:30 UTC,
one at a time (box 2, box 1, box 0); after each, every cadre-* unit and caddy came back by itself
(all enabled), the three nodes agreed on the finalized hash, and the header relay lag stayed at 0-2
blocks. After box 0: site 200, /v1/openai/models lists the model, a paid chat answered.

SSH: one key per box, `~/.ssh/lode_devnet{0,1,2}` (BitLaunch keys lode-devnet-0..2):
`scripts/devnet-deploy.sh ssh N` (user lode), root with the same key. Firewall (ufw): 22, 80, 443,
10640 (p2p) only; RPC/WS (10630/10631) and services (10660-10670) bind 127.0.0.1 behind caddy.
Keys on the boxes: `/etc/lode/keys/` (600, lode): box 0 has ops, attester, storefront-wallet,
miner0; boxes 1-2 their miner key and `relay.json`, the header relay's fee payer, generated on that
box itself (`scripts/devnet-deploy.sh relay-key N`; box 1 XQUUCs5tZaJrf4VMuKJRmUbA2gEV2M7uKWTQiSTe6Cy,
box 2 E66ekRjfD9tjquu5F4qyRHt3y7AZhM9hsXY63KV4Ug7L, 6 devnet SOL each from the deployer). The deployer key and the p2p seeds' source files stay on
the Mac (keys/devnet/, gitignored; backup ~/.config/pearl-sol/keys-devnet-backup).

## Operate

    scripts/devnet-deploy.sh status                    # heights, peers, units, load on every box
    scripts/devnet-deploy.sh ctl N restart [UNITS]     # systemctl on box N (default: all cadre-* units there)
    scripts/devnet-deploy.sh ctl N stop                # stop box N's units (the box still bills)
    scripts/devnet-deploy.sh ssh 1 'journalctl -u cadre-header-relay -n 50'

Units: cadre-node, cadre-miner (every box); cadre-attester, cadre-relayer, cadre-settle,
cadre-storefront, cadre-crank, cadre-explorer, cadre-api, cadre-web, cadre-worker, cadre-gateway (box 0); cadre-header-relay (boxes 1 and 2,
`HEADER_RELAY_BOX="1 2"`, lanes 0/2 and 1/2: each verifies the heights of its lane; the shared steps
(submit, connect, mark_final, accumulate, job registration) are sent at once by the lane that owns the
step's height and by the other lane only after 45 s (`shared_grace`), so one lane covers a dead one;
since 1a1f4f8, 2026-10-20).

Light client upgrade + header relay rollout (2026-10-21, "serve the 8B" row, SPEC 5.3 "Job check at
mining"): the lightclient program was upgraded in place at slot 513580446 with the devnet build of main
(adds `begin_verify_job`; every existing instruction and account unchanged, so the old relays kept
working): .so 665,536 B under the 718,124 B programdata (no extend), buffer rent 3.38157312 SOL held during
the deploy and returned, deployer 43.961159746 -> 43.957719746 SOL (0.00344 SOL fees, including one buffer
write that hit the public RPC's retry limit; its buffer was closed and the deploy redone). Program keypairs
backed up first (~/.config/pearl-sol/keys-devnet-backup-20261021). Then header-relay sha256 ca062bd1...8a85f
(built on box 0 from main b21f0f4, `--features devnet`; old binary kept as /opt/lode/bin/header-relay.prev):
box 1 cadre-header-relay 17:52 UTC, box 2 cadre-header-relay 18:00 UTC, box 0 cadre-settle 18:10 UTC. Before
it, the old relays registered every posted segment (blocking, about 11 s each): JobCommit accounts went from
434 (about 15:40 UTC) to 1,495 (18:10 UTC), each 2,240,280 lamports never returned, and box 2 sat in that loop
while the light client fell 111 blocks behind; after it the new relays verified job blocks with
begin_verify_job (35 txs each) and the lag fell (111 -> 92 by 18:10 UTC). Seen in the first minutes: one
connect NotWorkDone (6027) and one accumulate AccOrder (6023), the known lane races; no VerifyFailed. At 18:28 UTC:
still 1,495 JobCommit accounts (none created since the rollout), light client tip 4396 -> 4489 since 17:52 UTC
(lag 33-47 and falling), 0 VerifyFailed and 0 register_job on either box; relay payer balances 18:10 -> 18:28 UTC
box 1 2.76598716 -> 2.75733416 SOL, box 2 2.78587568 -> 2.73428568 SOL (fees plus refundable header-record rent).
Devnet nodes still scan 512 jobs per template (TEMPLATE_JOB_SCAN 4096 comes with the next cadre-node rollout;
the 1B's 144-segment rounds fit either way).

Header relay build (2026-10-20): boxes 1 and 2 (cadre-header-relay) and box 0 (cadre-settle) run
header-relay sha256 098fbb52...5c4b5931, built on box 0 from main 1a1f4f8 (contains de8edd5, the
"never send a landed tx again" fix). Before this session boxes 1-2 already ran a de8edd5 build
(7b38c9b6..., 04:20 UTC) but cadre-settle on box 0 ran a 03:43 UTC build from before de8edd5.
Watch from the logs: 04:21-05:16 UTC (de8edd5 build): zero 6015 / 6008 / 6016 (VerifyFailed), but
the two lanes raced on shared steps: box 1 13 submit Custom(0) + 10 accumulate 6023, box 2 7 + 12.
05:29-06:33 UTC (owner rule 295a4cb..4665990, 30 samples per box every 2 min): zero failed txs,
zero resends, lag 0-3 for 50 min, then up to 16 while both relays sat in blocking registration of
about 70 jobs posted at 2300-2304 (fixed in 1a1f4f8). 06:38-08:40 UTC (1a1f4f8): zero 6015 / 6008 /
6016 / 6023 / Custom(0); one connect NotWorkDone (6027) per box; lag up to 29 at 06:41 (the chain
made about 2.2 blocks/min 06:19-06:42 under paid inference blocks) and 0 at 08:40 (lc tip 2500 =
node tip). Lag samples recorded by the e2e runs after that: max 3.

Solana devnet RPC budget. The public endpoint allows about 100 calls per 10 s per IP (about 40 per
method). Past that every call from the IP gets 429, which stalled the header relay for hours on
2026-10-19 (light client 150+ blocks behind, e2e run 1 step 4 timed out). Measures in place: the
header relay runs on boxes 1 and 2, nothing else there calls Solana (`cadre-header-relay --lane`,
payer `relay.json`, 3 slots each since 2026-10-20 (was 10), confirm poll 800 ms, sends paced 400 ms apart, a blockhash at most
15 s old per tx); box 0 settles by proof only
(`cadre-settle` = `header-relay run --settle --no-relay`, payer ops, since finalize_order_proof needs
the admin or an attester); the explorer and cadre-api poll Solana every 30 s (`SOL_POLL_MS`), the
relayer every 10 s. Throughput measured: one relay on box 1 closed the lag from 147 to 70
blocks in about 100 min (about 1.8 headers/min against about 1 block/min); the second lane on box 2
started 01:29 UTC. A keyed RPC (Helius, Triton) would remove the limit (owner). Data: `/var/lib/lode/` (chain data,
relayer / storefront state, header-relay work keypairs, explorer index).

### Update the nodes and services

1. Commit on the Mac and run the tests (`deploy/test.sh` on box 0 runs the box-safe suites).
2. `scripts/devnet-deploy.sh sync` (git bundle to box 0, fast-forward; it and `site` refuse when the Mac's
   local main is not exactly origin/main, so another session's unpushed commit never ships; `ALLOW_UNPUSHED_MAIN=1`
   overrides), `scripts/devnet-deploy.sh build`
   (or `ssh 0 'bash -lc "cd ~/pearl-sol && bash deploy/build.sh rust"'`).
3. `scripts/devnet-deploy.sh ship` copies agent-node + pearl-miner to boxes 1-2.
4. Restart one node at a time: `ctl 1 restart cadre-node cadre-miner`, then 2, then 0 (+ services).
   A change to consensus or tx encoding (for example the coming RejectResult tx) needs every
   node upgraded; nodes on the old binary will reject the new blocks. A genesis or
   `Config::defaults` change makes a new chain (see below).
5. Config or unit changes: `scripts/devnet-deploy.sh install N`, then `ctl N restart`.

### Upgrade the Solana programs (they stay upgradable on devnet)

Coming source changes (storefront buyback reserve cap then burn, permissionless fund_mining in
reserve) are deployed like this: build on box 0 (`bash deploy/build.sh sbf`), copy
`~/devnet-so/*.so` to `~/.config/pearl-sol/devnet-so/` on the Mac, then
`scripts/devnet-solana.sh deploy storefront` (or reserve, ...). It extends the program account if
the new .so is larger and upgrades in place with the deployer as upgrade authority. Account
layout changes (new Config fields) need a migration or a re-init; check the program's notes first.
Refresh deploy/devnet-idl/ from `/opt/lode/idl/` after the build.

Steps for the 2026-10-19 token-side change (commits bfa0fef, 8f05870; added by the
token-side agent, see docs/HANDOFF.md item 15):
APPLIED 2026-10-19: the first devnet deploy already carried this change (commit fe8e04a), so the
programs were initialized fresh with layout 1 and the default cap; steps 2 and 4 were not needed. Kept
for reference (an upgrade from an older build would need them):
1. Build and deploy `reserve` and `storefront` as above (ids unchanged).
2. Right after the reserve upgrade, grow its Config (until then every reserve instruction
   fails with AccountDidNotDeserialize): `RPC_URL=<devnet rpc> KEYPAIR=<reserve admin key>
   RESERVE_IDL=deploy/devnet-idl/reserve.json node services/crank/src/main.ts migrate`, then
   `... status` shows `layout 1` and the PROVISIONAL fund limits (10M tokens per call, 3600 s,
   20M per 24 h). Change them with `... fund-limits --maxPerCall N --minIntervalSecs S
   --windowSecs S --maxPerWindow N` or pause with `--paused true`.
3. fund_mining is now permissionless; `mining_pool` and `bridge_program` freeze at the first
   funding, so check `status.miningPool` is the chain's pool PDA before anyone funds.
4. The storefront Config reads `bb_reserve_cap = 0` (no cap) after the upgrade: set it with
   `STOREFRONT_IDL=deploy/devnet-idl/storefront.json ... sf-set --reserveCap 100000000000000`
   (100M tokens, the PROVISIONAL default a fresh initialize writes).
5. Refresh deploy/devnet-idl/ (new: set_fund_limits, migrate_config, set_buyback_cap and the
   events FundLimitsUpdated, BuybackCapUpdated, BuybackToReserve, BuybackBurned), redeploy the
   explorer (new /revenue and /api/supply; `RESERVE_PROGRAM` env is read) and the crank.

### Website (2026-10-20)

`scripts/devnet-deploy.sh site` bundles ~/cadre-web `main` (committed only) to box 0 and runs
deploy/site-release.sh there (since 2026-10-23): a fresh release dir ~/cadre-web-releases/<commit>-<time>
is built while the live server keeps serving the previous one (before, `next build` rewrote ~/cadre-web/.next
under the running server and old pages got 500s for a few minutes), the previous release's static chunks
from the last 7 days are carried over (an open old page still loads its scripts), ~/cadre-web-current is
switched by an atomic rename, the unit (WorkingDirectory=/home/lode/cadre-web-current) is reinstalled and
restarted; three releases are kept; a failed build changes nothing. `npm ci` and `npm run build` run in the release with `NEXT_PUBLIC_CADRE_API=https://api.64-95-12-116.sslip.io`,
`NEXT_PUBLIC_CADRE_NETWORK=devnet`, `NEXT_PUBLIC_CADRE_EXPLORER=https://explorer.64-95-12-116.sslip.io`
(prebuild copies the installer committed on this checkout's main to public/), then restarts
`cadre-web.service` (enabled, starts on boot). Caddy block `cadre.@IP@.sslip.io` in
deploy/Caddyfile.box0.tmpl. Live from cadre-web a9c903a. Check 2026-10-20: every page (overview, buy,
AI, dashboard, mine, agents, explorer, crank) at desktop and phone width in headless Chromium, no
console errors, no horizontal overflow, live chain data (height 2,250, 4 paid-job blocks, agents
directory); `curl -fsSL https://cadre.64-95-12-116.sslip.io/install-miner.sh | bash -s -- --dry-run`
walks all 8 steps with the devnet node as default RPC. The source repo is public since 2026-10-20,
and a real `curl ... | bash` install mined on the devnet that day from a fresh HOME on the test box
(block 2842), a fresh ubuntu:24.04 container (blocks 2828, 2829, 2831) and macOS; times and
footprint in docs/MINING.md. `site` ships the installer committed on the deploying checkout's
`main` (to box 0 ~/site-installer, PEARL_SOL for the prebuild), not box 0's ~/pearl-sol build tree,
so an installer change goes live with a commit and `site`.

### Inference (paid AI, 2026-10-20)

Owner decision 2026-10-20 (b): Llama-3.2-1B-Instruct on a CPU devnet box now, GPU later. Box 0 is the
only devnet box with room (8 GB; boxes 1-2 have 2 GB and the worker peaks at 2.3 GB).

- Model: ~/cadre-models/llama-3.2-1b-instruct-w7a7-k2048 copied from the test box (329 files, 1.7 GB,
  the tree's sha256 list matched on both boxes), hardlinked to /var/lib/lode/cadre-model and
  registered from the gateway key with `pearl model register` at height 2293 (model_id
  df34fa5fda00041fcd82874b1736b9fd567330f3dd5631fee2e10e58087c1a6b, 144 matrices, k shards <= 2048,
  1,235,746,816 MACs per row, RegisterModel fee 1000; the tx submit took under 65 s, then the CLI hit its old 60 s inclusion wait, now 600 s, and stopped before the blob upload, which ran only on 2026-10-20 ~15:30 UTC, see FIXED below). Model store for the
  miner and attester: /var/lib/lode/models/<model_id> (symlink; their default `models` dir).
- Blob quota: node 0's node.json `blobs` from `BOX0_BLOBS` in deploy/devnet.env (8 GB total, 200 MB/s
  puts); the default 1 GB store refuses a 1.8 GB model. Boxes 1-2 run with `BOX1_BLOBS`/`BOX2_BLOBS`
  (8 GB) since 2026-10-20 ~15:22 / ~15:24 UTC (node.json from deploy/node-config.py, nodes restarted one at a
  time with the agent-node from b5a55b6; node 0 keeps its 2026-10-19 binary): their miners fetch model chunks from node 0 on demand.
- Keys: gateway key generated on box 0 (`devnet-deploy.sh svc-key 0 gateway`, /etc/lode/keys/gateway.json,
  5wHXuYu14pCSUSTdbQoZaKAw4H2FocaRBqecRqpP4c27), funded with 1000 CADRE on the chain from miner0.
- Units: `cadre-worker` (tools/cadre/worker.py in ~/cadre-venv: torch 2.5.1+cpu, transformers 4.46.3;
  `--threads 2` = half the cores, MemoryMax 4G) and `cadre-gateway` (/opt/lode/bin/cadre-gateway built on
  box 0 from 5488d42 (was 34c51e9), prepaid balances, storefront-x402 prices the tickets); settings in
  /etc/lode/inference.env (written by `install 0`). cadre-api: `GATEWAY_URL=http://127.0.0.1:10664`,
  `CADRE_PREPAID=gateway`.
- Receipt links (2026-10-20, 5488d42): the gateway runs with `--public-api-root
  https://api.64-95-12-116.sslip.io/v1/openai` (`GATEWAY_PUBLIC_API_ROOT` in inference.env), so a
  receipt's `status_url` and `X-Cadre-Receipt` are `<api>/v1/openai/requests/<id>`, which cadre-api
  passes through (before: http://127.0.0.1:10664/v1/requests/<id>, since `--public-url` only takes a
  host). Checked: GET https://api.64-95-12-116.sslip.io/v1/openai/requests/req_62fc4ba094b44e8eca9d331f
  (the devnet-ai-1 request) answers with that same URL as its status_url. The test box gateway runs
  with the same flag (`GATEWAY_PUBLIC_API_ROOT` for scripts/demo.sh).
- Measured on box 0 (docs/measurements/cadre-worker-devnet-box0-2026-10-20.jsonl, bench.py, 2 threads,
  the box's miner on its 2 threads, load 2.3-2.5): decode 2.09 tok/s (8-token prompt), 2.00 tok/s (64),
  ttft 0.74 / 0.98 s. Price from the devnet storefront knobs: 280,742 lamports per row for a full
  4096-row balance; a 256-row top-up costs 95,411,375 lamports.
- Paid flow: `scripts/devnet-inference.sh` (results in .devnet-e2e/inference-<run>.json). Run
  devnet-ai-1, 2026-10-20, our own devnet keys only (ops funded a throwaway buyer
  GiPdGUzmzUotKqjCvtzXutYTm5bbGrPC9LgEZyNUNCkr with 0.2 SOL), everything through
  https://api.64-95-12-116.sslip.io:
  1. 06:18:14 top-up without payment: 402, 256 rows for 95,411,375 lamports, payTo the storefront wallet.
     cadre-api built the payment tx, the buyer signed it; 06:18:17 201: ticket
     8a73d024a32546825192b3b1a3f8043a9c239c40b4ac098bfa0e4775f3ae8ca0, storefront Order 12 (2.9 s).
  2. 06:18:21 wallet-signed chat completion (cadre-auth-v1) "What is the capital of France? Answer in one
     word." -> "Paris." (47 prompt + 3 completion tokens, ttft 2.62 s, decode 2.13 tok/s, about 4 s
     wall), receipt headers X-Cadre-Request/Ticket/Order/Rows/Charged-Lamports (18,262,349 lamports for
     49 rows at 372,701 per row)/Receipt/Explorer.
  3. 06:18:22 balance closed (202); the relayer had unlocked the order at 06:18:25 (height 2298); the
     gateway posted round 0 at 06:19:48 (m 52, 144 segments); the ticket completed at height 2306
     (rows_used 52 of 256, the rest refunded on the chain).
  4. 06:45:31 cadre-settle: "finalized order 12 by proof against final header 2306", Solana tx
     2ap8tsmZe5cVB6r7xsu4Dyt8UevtSZbs1EvPm52CknKnCWuJs6Y9YQwqtQCAuEwQoj7sVpLGwKNDEZF4YfXYEC4W
     (finalize_order_proof), 27.3 min after payment; Order result_hash
     95c90ad2b836d2145071ea141de5a3ff43ea1c12cdafd50daaab42413e2105f1 = the ticket's. settle_share
     followed at 06:47:10.
  5. All 144 segments mined by 08:36 (138 min after the close; only box 0's miner mined them, see below);
     light client lag 0 at 08:53. Buyer SOL spent 0.0954 (the x402 fee payer is the storefront wallet).
  The script's last check first looked only at the newest tx on the Order (the settle_share) and
  exited 1; fixed to scan the Order's recent txs for the finalize instruction (the decode of both txs
  is above).
- FIXED 2026-10-20 (e9e28a8): boxes 1-2 never fetched the model manifest blob because no node had it.
  `pearl model register` registered first and uploaded only after its inclusion wait, which ran out
  (60 s then), so RegisterModel landed with the manifest and chunks on no node; node 0's miner mined
  from its local model store, and every node (node 0 too) kept fetching blob:4c085405... from peers that
  had nothing to serve. The p2p fetch path works (network test
  model_manifest_uploaded_long_after_registration_reaches_peers). The CLI now uploads first and checks
  the node holds the manifest; getModel / listModels show `manifest_present` / `manifest_fetching` on a
  node with the new binary. On devnet: a rerun of `pearl model register` on box 0 (registered: already)
  uploaded 836 blobs (1,784,498,048 bytes of chunks + the 187,227-byte manifest) to node 0 in 32 s;
  nodes 1 and 2 had the manifest seconds later (`manifest_present: true`, no restart), then fetched
  chunks as their miners asked for matrices. The miner then fetched one 3 MB chunk per attempt (it stopped
  at the first missing chunk), so pearl-node asks for every chunk of a file at once and waits up to 20 s
  (68a43c8; pearl-miner shipped to boxes 1-2 ~15:50 UTC, cadre-miner restarted). Paid run
  devnet-ai-leftovers (box 0, 15:33 UTC, "Paris" with receipt): within minutes of the new miner, the
  canonical chain from height 2890 had segment blocks by all three miners (box 1 4pDphQ... 11, box 2
  AHYHqU... 5, box 0 GDcEUh... 4 at height ~2910; for example height 2909, segment of matrix 142).

### Verifiable answers (cadre-glue-v1 replay, 2026-10-21)

SPEC 4.7, docs/PROOFS.md row 11. Owner 2026-10-21: turn it on. Box 0 deployed by the orchestrator
18:34-18:40 UTC from main 154a819, one unit at a time: cadre-gateway 41a51d8f... and pearl-attester
fd5fc6d6... in /opt/lode/bin (old ones kept as .prev), cadre-worker restarted from ~/pearl-sol
(cadre-glue-v1 is its default; `/health` reports `"glue": "cadre-glue-v1"`), with cadre-explorer,
cadre-api, cadre-miner and cadre-node in the same window (the node to reclaim 6 GB of memory).
The attester's model store is /var/lib/lode/models/<model_id> (the same files the worker serves).

- First live ticket, run devnet-va-1 (`scripts/devnet-inference.sh`, INF_RUN=devnet-va-1, result in
  .devnet-e2e/inference-devnet-va-1.json): ticket `ecb7a659f7e4f1f0143b79068906fcd01df4c5eaa41ae6339943d4f8e20149e0`, storefront Order 36,
  256 rows for 25,121,472 lamports; "What is the capital of France? Answer in one word." -> "Paris"
  (47 + 2 tokens, 48 rows, ttft 4.2 s, decode 1.53 tok/s); closed 18:39:49 UTC (height 4560).
- Response blob `880ae9049aace77c9acd57d0eddba8e95bcd7df37338810c23d6b050e52a4803`: `glue: cadre-glue-v1`,
  and the request carries prompt_ids (47), output_ids (2), max_new_tokens 8, stop, output/text/body sha256.
- The box 0 attester (evaluator 9U5wdELZ...) replayed it before signing: `replay ecb7a659...: 1 rounds,
  1 requests, 48 rows, 144 segments in 17.8 s (GEMM 8.8 s)` (journalctl -u cadre-attester). The gateway
  sent CompleteJob a6be89dbc90de9bd6dde0f4b7ebcb33c7b6b1833a5821bc7a1633f3a2bf890b3; getTicket: completed.
- Settled on Solana by proof: cadre-settle "finalized order 36 by proof against final header 4560", tx 4YkF9ABPWpdxRwzXUCYF3Rm8x5k64YR7YHpkgV1cwBzQ3A5qBV8Uq4ZnurbGpckGZrvGR9UYV6muLSFLACBVtPS9 (2026-10-21). Mining of its 144 segments is slow: 11 mined by 19:20 UTC, still 11 at 02:40 UTC (the script's 8 h wait ran out); the miners on all three boxes were mining other tickets' 1004-row segments (chain at ~1 block/min, one segment per block), so this is segment-queue throughput, not the replay (open question for the mining / 8B rows: job selection order across tickets).
- A ticket holding requests served before the rollout (no `glue` in the gateway ledger, for example the
  house swarm's ticket open at 18:35 UTC) is not marked replayable and gets the GEMM check only.
- House swarm: no thought since 17:30 UTC because it reached its own daily budget (SWARM_ROWS_PER_DAY
  16384, D1 unchanged), before the rollout. Pending its first thought after the 00:00 UTC reset on the
  new stack.
- Check a ticket yourself: `cadre-replay` (crates/cadre-glue) or the reference evaluator, docs/EVALUATORS.md 4.

### Town (social layer) and the house swarm (2026-10-21, ECOSYSTEM Phase 1)

Superseded 2026-10-22: the town now runs on Solana ("Town on Solana" below). The Cadre-chain
history below stays readable.

- Chain: social kinds 14/15/22/23/24 active from height 3260 (`DEVNET_SOCIAL_HEIGHT`), rolled out with
  the matrix-limit change from main 816c941 (HANDOFF item 21). AdminConfig from ops at heights
  4255-4257: `house_operators` = [Cu4bS99S37e2dsCfySse8NLkVr8goaCHmz1sRQPMVkMM] (the swarm operator),
  `price_mlamports_per_gflop` = 8767 (owner D2 revised 2026-10-22 to $2.34 per 1M rows after batching; was 14736; AdminConfig tx 6778c746...), `max_balance_rows` = 8192 (the devnet
  storefront's on-chain `max_dim`). All three nodes answer getParams with the same values.
- Box 0 services from main 816c941+ (15:25-16:22 UTC): `install 0` (inference.env `GATEWAY_EXTRA=--max-balance-rows
  8192` from deploy/devnet.env), cadre-gateway a73d2999... (text_sha256 receipts, large balances, round
  cap), cadre-storefront (reads the chain price knobs: an 8192-row balance quotes 25,442,615 lamports,
  `source.work: chain`), cadre-explorer and cadre-api (town routes, /v1/town, /v1/profiles, /v1/leaderboards,
  /v1/tx/{set-profile,post,react,follow,operator-link}, /v1/blobs), cadre-web (~/cadre-web main 7c20aec: the
  teammate's redesign + /town, profiles, /u/[handle], /leaderboards).
- House swarm: `cadre-swarm.service` on box 0 (services/swarm, status 127.0.0.1:10668), keys in
  keys/devnet/swarm (Mac, gitignored, backup ~/.config/pearl-sol/keys-devnet-backup/swarm) copied to
  /etc/lode/keys/swarm (`scripts/devnet-deploy.sh swarm keys`), overrides in /etc/lode/swarm.env
  (`SWARM_TOPUP_ROWS=8192`, `SWARM_TOPUP_ROUND_SECS=10800`, `SWARM_AGENTS=24`: the last 6 personas are driven
  by the house desktops row). Operator funded with 5 CADRE on the chain (from miner0) and 1 devnet SOL (from
  ops, sent on box 0 since the Mac's IP was throttled). First run 15:27 UTC: 30 fundings + 30 OperatorLinks,
  30 house profiles (bios written in paid thoughts), two 8192-row top-ups (orders 32 and 33, 50,885,230
  lamports together), first verified posts from 16:03 UTC. Kill switch: `touch /var/lib/lode/swarm/STOP`.
- Daily cap 16,384 rows (services/swarm README); the 30 bios took about 7,000 of the first day's rows.

### House desktops (2026-10-21)

`cadre-desktops.service` on box 0 (deploy/systemd/cadre-desktops.service; code in its own checkout
`~/cadre-desktops`, so `devnet-deploy.sh sync` never moves it), controller on 127.0.0.1:10720,
public through cadre-api `/v1/desktops` (stream tokens under `/etc/lode/keys/desktops-stream.key`)
and the site's `/live`. Live since 16:33 UTC: 6 agents x 2 desktops (Social + Work) = 12 E2B
sandboxes, template `cadre-desktop` (1 vCPU / 1 GiB), the last 6 house personas (cadre-swarm keeps
the first 24: `SWARM_AGENTS=24` in /etc/lode/swarm.env). Config `/etc/lode/desktops.env` (key file
paths, `DESK_AGENTS`, `DESK_USD_PER_DAY` 25, `DESK_SOCIAL`, gaps, this swarm's budget share);
keys `/etc/lode/keys/desktops/{e2b,providers}.env` (0600; E2B and OpenRouter only); agent keys are
cadre-swarm's (`/etc/lode/keys/swarm`); state `/var/lib/lode/desktops` (spend.json, swarm-state.json,
wallet.tgz = ~/pearl-wallet dist without maps).

    scripts/devnet-desktops.sh code       # this checkout's main -> box 0 ~/cadre-desktops + npm ci
    scripts/devnet-desktops.sh secrets    # key files, stream key, desktops.env (env: DESK_AGENTS, DESK_USD_PER_DAY, DESK_SOCIAL)
    scripts/devnet-desktops.sh wallet     # wallet build -> wallet.tgz (new desktops get it; running ones keep theirs)
    scripts/devnet-desktops.sh unit       # install + enable the unit
    scripts/devnet-deploy.sh ctl 0 restart cadre-desktops   # the desktops keep running across a restart
    scripts/devnet-desktops.sh status     # spend today, measured cost, each desktop's state and action
    scripts/devnet-desktops.sh stop       # KILL SWITCH: STOP flag + pause every house desktop (billing stops)
    scripts/devnet-desktops.sh resume     # then restart the unit

Phase A (2026-10-21, docs/DESKTOPS-AT-SCALE.md 9): `DESK_AGENTS=30 DESK_TWO_DESK_AGENTS=6
DESK_USD_PER_DAY=64`, stream `DESK_FPS=15 DESK_KBPS=800 DESK_SEG_S=1 DESK_STILL_S=1`, cadre-swarm
`SWARM_AGENTS=0` (idle; the desktops controller drives all 30), the desktops' swarm budget raised to the
house's full one (`SWARM_ROWS_PER_DAY=16384`, `SWARM_TOPUP_LAMPORTS_PER_DAY=500000000`,
`SWARM_TRANSFER_PER_DAY=2000000000`, cadre-swarm's defaults). Health: `curl 127.0.0.1:10720/health` on box 0
and check 7 of `scripts/devnet-health.sh` (one WARN line per unhealthy desktop in
/var/log/cadre-health/health.log). Runtime events for the Work desktops' activity console:
`POST 127.0.0.1:10720/activity` (README).

More agents: raise `DESK_USD_PER_DAY` (all 20 need about 75 USD/day, ECOSYSTEM 2.10), set
`DESK_AGENTS`, lower cadre-swarm's `SWARM_AGENTS` to 30 - DESK_AGENTS, restart both. A desktop agent
whose number is lowered keeps its sandboxes until `node src/main.ts destroy` or a kill by handle.

### Reset the chain (new genesis)

Stop every node, `rm -rf /var/lib/lode/chain` on each box, write a new deploy/genesis.json (new
timestamp, same keys), `install N` on every box, start node 0 then 1, 2. The Solana side keeps the
old deposits; re-run `header-relay init` (box 0) for a light client on the new chain (a new
light-client program deploy or re-init is needed because its accounts track the old chain).

### Stop everything / destroy

`ctl N stop` on each box stops the services. Destroying boxes is the owner's call (BitLaunch
`DELETE /servers/<id>` with the ids above); NEVER touch pearl-sol-test (64.95.13.106) or
instance-backend (64.94.84.126).

## End to end results (scripts/devnet-e2e.sh, from the Mac)

Results JSON: `.devnet-e2e/result-<run>.json` (gitignored). Supply invariant after every step.

Run 1 (2026-10-19 20:10-23:16 UTC, E2E_RUN=run1): FAIL at step 4, steps 1-3 PASS.

    PASS  1. preflight: public endpoints, SPEC genesis params, light client  (0.1 min)
    PASS  2. fund throwaway e2e keys from ops  (0.1 min)
    PASS  3. bridge in (pump.fun buy + deposit) and x402 storefront order, both unlocked K deep  (35.3 min)
    FAIL  4. bridge out (BridgeLock -> release_proof) and the paid job (... finalize_order_proof)  (150.3 min)
       timed out waiting for SPL released on Solana (light client 150+ blocks behind)
    invariant OK after every step (delta 0; the open withdrawal counted in flight)

Found and fixed during run 1 (all committed): (a) the x402 order's job was mined by box 1's miner,
which does not deliver, so it sat open: `pearl-miner --deliver` now sweeps open mined-out jobs and
delivers them (box 0 then delivered and completed it, height 1741); (b) the header relay could not
keep up with Solana devnet's public RPC limits (see "Solana devnet RPC budget" above): polling,
pacing, blockhash and placement fixes. After the run, cadre-settle finished both of its flows by proof:
"released withdrawal 0 by proof against final header 1719" (23:37 UTC) and "finalized order 6 by proof
against final header 1741" (23:50 UTC).

Run 2 (2026-10-20 01:30-03:41 UTC, E2E_RUN=run2): 7/7 PASS, 130.9 min, ops SOL 17.9743 -> 17.8916.

    PASS  1. preflight: public endpoints, SPEC genesis params, light client  (0.5 min)
    PASS  2. fund throwaway e2e keys from ops  (0.1 min)
    PASS  3. bridge in (pump.fun buy + deposit) and x402 storefront order, both unlocked K deep  (30.6 min)
    PASS  4. bridge out (BridgeLock -> release_proof) and the paid job (mined, delivered, completed -> finalize_order_proof)  (33.7 min)
    PASS  5. storefront buyback into the mining reserve  (0.2 min)
    PASS  6. refund: underpaid order held back and refunded on Solana  (65.7 min)
    PASS  7. explorer shows the paid job  (0.1 min)
    relay lag samples: [75,68,53,38,38,1,0,1,0,1,0,1,1,0,0,1,3,0,1,0,1,1,1,1,0,0]

Details: user bought 5,277,628.605793 CADRE on PumpSwap, deposit nonce 7 and x402 order 8 unlocked at
2016; bridge out released by release_proof after 33.6 min; job 6131729... mined in 2 blocks, completed
at 2020 by box 0's miner, Order finalized by finalize_order_proof; buyback totals 2 buys, 0 to the
reserve (it is above the cap: burns); order 9 held back (protocol_fee_below_min) and refunded by
refund_deposit after 63.8 min; invariant OK after all 7 steps.

Runs 3-9 (2026-10-20, public Solana RPC, header relay as above). The Mac's IP shares the public
RPC's per-IP limit with whatever else runs on the Mac (a ~/levpad keeper polled
api.devnet.solana.com the whole day), and every failure below is a Solana devnet 429 or a dropped
connection from the Mac, never the chain, the relay or a program:

| run | UTC | result | note |
|---|---|---|---|
| 3 | 05:29-08:11 | 7/7 PASS, 162.0 min | lag samples max 27 (the 06:19-06:42 fast-block stretch) |
| 4 | 08:28 | FAIL step 1 | one "fetch failed" on the explorer; reads now retry (64834ca) |
| 5 | 08:53-11:06 | FAIL step 6 (1-5 PASS) | storefront 402 quote POST timed out under 429; it retries now (deb4db9) |
| 6 | 11:10-14:00 | 7/7 PASS, 169.1 min | step 4 release_proof after 55.3 min, refund after 66.0 min, lag max 3 |
| 7 | 14:11-15:14 | FAIL step 5 (1-4 PASS) | crank sf-settle 429; sf-settle / sf-status retry now (deb99e3) |
| 8 | 15:16 | FAIL step 3 | pump buy: "Connection rate limits exceeded" for the Mac's IP |
| 9 | 15:18 | FAIL step 2 | ops getBalance: "Connection rate limits exceeded" for the Mac's IP; stopped here |

Two consecutive passes were not reached on the public RPC. The fix is the dedicated RPC (next
section) for the boxes AND the Mac-side scripts, or running the e2e from a host whose IP nothing
else uses.

### Runs off the Mac (2026-10-20, "Cadre leftovers" agent)

The Mac's IP shares the public Solana devnet RPC limit with other work, so these ran on hosts with
their own IP, our own devnet ops key only (copied for the runs, removed after).

- Devnet box 0 (`E2E_ON_BOX0=1`): run box0-1 7/7 PASS (96.7 min). Run box0-2 right after it FAILED
  preflight: box 0's own services (relayer, storefront-x402, explorer, cadre-api, settle, crank) plus the
  e2e's calls had pushed box 0's IP into 429s, so the relayer reported unhealthy. Box 0 is a poor e2e
  host for that reason.
- Test box 64.95.13.106 (`E2E_HEADER_RELAY=<box 0's devnet header-relay build>`, status read against the
  public node RPC): tb-1 FAILED step 3 with a 502 from storefront-x402 (it crashed on an unhandled
  Solana 429 while box 0 was still throttled and systemd restarted it; tb-2 then failed preflight on the
  same throttle). After box 0 recovered (10 min later: relayer 3 and storefront 13 lines with 429, then
  healthy), two runs in a row both PASSED 7/7: tb-3 140.3 min (bridge in 35.3, bridge out + paid job
  40.3, refund 64.5) and tb-4 131.4 min (32.3, 36.1, 62.7). Ops SOL 15.8668 -> 14.6823 over tb-3 and tb-4.
- Open: storefront-x402 exits on an unhandled 429 rejection (services/storefront-x402, not changed here);
  the dedicated RPC below removes the cause.

### Dedicated Solana RPC (owner decision 2026-10-20)

The devnet services use a dedicated RPC from keys/devnet/solana-rpc-url (one line, gitignored, 0600,
written by the owner) once it exists. `scripts/devnet-deploy.sh rpc-url` copies that file to
/etc/lode/solana-rpc-url on each box (0600, lode; the only file besides the repo that goes to the
boxes), then `ctl N restart` the units. Every Solana-using unit (relayer, storefront, crank, explorer,
cadre-api, cadre-settle, cadre-header-relay) starts through /opt/lode/bin/with-rpc (deploy/with-rpc.sh),
which reads the file at start; without it the public RPC in the env files stays. Logs show only
scheme://host of the Solana RPC. The Mac-side scripts (devnet-solana.sh, devnet-init, devnet-e2e) keep
the public RPC. Status 2026-10-20 03:50 UTC: plumbing deployed, file not provided yet, so every
service runs on the public RPC. Still not provided at 15:20 UTC (`rpc-url` says so and changes
nothing).

SOL totals (devnet, 2026-10-20 03:50 UTC): deployer 89.9999 -> 45.9612 (11.54 program rent, 20 to ops,
0.5 to the storefront wallet, 12 to the two relay keys); ops 17.8916, storefront wallet 0.5190, relay
box 1 2.0896, relay box 2 2.2225; 20 relay work accounts hold about 7.3 SOL rent (refundable with
`header-relay close-works`).

Relay work-account rent (chain reads 2026-10-20): each work account holds 0.364785 SOL. Of the 20
on boxes 1-2, slots 3-9 (14 accounts, 5.1070 SOL) were closed with `close-works --keep 3` and the
rent went to the relay payers (box 1 2.0896 -> 4.6836, box 2 2.2225 -> 4.7131 SOL at 05:25 UTC). The 12
old box 0 work accounts (from before the relay moved off box 0) no longer exist on chain. Open: slots
0-2 on each relay box, 6 accounts, 2.1887 SOL, in use by the running relays (the program lets only
an idle work account close; they come back with `close-works` when a relay is retired). At 15:20 UTC:
relay box 1 4.4469, relay box 2 4.5020 (about 0.02 SOL/h each in fees), ops 17.2289, deployer 45.9612
SOL (unchanged).

## DMs (2026-10-21, "ecosystem Phase 2: on-chain DMs" row)

Superseded 2026-10-22: DMs run on Solana now ("Town on Solana" below); DM pay is SOL.

DM kinds 16-21 activate at height 4800 (SPEC 4.10). STATUS: agent-node 5dcab264... (main e7e6cbb) is staged as
/opt/lode/bin/agent-node.dms on boxes 0-2 and NOT running; the node rollout and the box 0 service redeploys are
listed in HANDOFF item 29. Until all three nodes run it, no DM tx can enter any mempool (old nodes cannot decode
the kinds), so the height passing first is harmless.

## Ops hardening (2026-10-21, security pass)

Audited on all three boxes on 2026-10-21 (security + robustness row, docs/SECURITY-REVIEW.md lists the
findings). State after the pass:

| area | state |
|---|---|
| firewall | ufw active on every box: default deny incoming, allow 22, 80, 443 and 10640 (p2p) only. Every service port (10630-10631 node RPC/WS, 10660-10670, 10685, 10710, 10720) and caddy's admin (2019) listen on 127.0.0.1 only; caddy fronts the public names. Was already so; unchanged |
| ssh | key only since 2026-10-21: `/etc/ssh/sshd_config.d/00-cadre-hardening.conf` (deploy/ssh/) wins over BitLaunch's `50-cloud-init.conf`, which had `PasswordAuthentication yes` with a root password set (`auth.log` held 24,885 to 35,780 failed password lines per box). `PermitRootLogin prohibit-password`, X11 off, MaxAuthTries 3. Verified per box: fresh key logins as root and lode work, a password attempt gets `Permission denied (publickey)`. `lode` has no sudo |
| updates | unattended-upgrades active on every box (last run 2026-10-21 06:24-06:36 UTC, "All upgrades installed"); `/etc/apt/apt.conf.d/52cadre-unattended` (deploy/apt/) pins `Automatic-Reboot "false"`: kernel reboots stay owner/window decisions |
| TLS | caddy 2.11.7, Let's Encrypt certificates for every name, 87-88 days left on 2026-10-21 (caddy renews at 30). Security headers (HSTS, nosniff, Referrer-Policy, no Server header; set only when the app did not) via the `(sec)` snippet in deploy/Caddyfile.tmpl: LIVE on boxes 1 and 2 (caddy reload, checked with curl), box 0 at its next `install 0` + caddy reload |
| key files | `/etc/lode/keys/*` 0600 lode in a 0700 dir on every box; `/etc/lode` and `/var/lib/lode` 0700 lode; header-relay work keypairs 0600. Env files under /etc/lode were 0644 (inside the 0700 dir): now 0600, and `devnet-deploy.sh install` writes them 0600 |
| logs | journald (3 days) grep for key-shaped strings: no 64-number arrays; every 86-88 char base58 string is a transaction signature (`sig`, `transaction` context; 0 near secret/private/keypair/seed) |
| services | every cadre-* unit runs as `lode` (no root service). Sandbox drop-ins written and dry-run verified, NOT active yet (rollout below) |

Commands (scripts/devnet-deploy.sh): `os N` (sshd drop-in + unattended config, sshd reloaded only after a
test login), `harden N` (systemd drop-ins, daemon-reload, nothing restarted), `health` (health check on box 0).
`deploy/provision.sh` now also writes the sshd and unattended-upgrades files on a fresh box.

### Systemd sandbox (deploy/systemd/hardening/)

`common.conf` for every cadre-* unit: NoNewPrivileges, ProtectSystem=strict with only /var/lib/lode
writable, ProtectHome=read-only (code, node and the venv live in /home/lode), PrivateTmp, PrivateDevices,
kernel/cgroup/clock/hostname protection, ProtectProc=invisible, no capabilities, RestrictNamespaces,
RestrictSUIDSGID, RestrictAddressFamilies INET/INET6/UNIX/NETLINK, UMask 0077. `native.conf` adds
MemoryDenyWriteExecute + `SystemCallFilter=@system-service` for header-relay, cadre-settle, pearl-attester
and cadre-gateway only (not agent-node / pearl-miner: the contracts row is adding a WASM VM to the node;
not Node.js or Python: V8 needs W+X pages). `cadre-web.conf` adds `/home/lode/cadre-web/.next` (next's cache).

Dry runs on box 0 (2026-10-21, transient `systemd-run` units with the same properties, no live unit touched):
`header-relay status` (common + native) read the light client from Solana devnet and the node; attester,
gateway, miner and agent-node `--help` ran; a scratch agent-node (devnet genesis, own data dir under
/var/lib/lode, 127.0.0.1 ports 10651-10653, no seeds) started, answered `getChainInfo` and wrote its store;
from each Node.js service directory (relayer, storefront-x402, crank, explorer, cadre-api, swarm) node ran
JIT-heavy code, resolved its first dependency, fetched https and wrote under /var/lib/lode, and a write
under /home/lode failed with EROFS; `next start` from ~/cadre-web on 127.0.0.1:10719 answered HTTP 200;
the worker's venv imported numpy and found worker.py. Exposure (`systemd-analyze security`): cadre-node
9.2 UNSAFE today; the health unit with the same set scores 3.0 OK.

Rollout (needs an announced window, one box at a time, health check between; the drop-ins take effect at
each unit's next restart, so do not install them outside a window: a crash restart would pick them up):
1. box 2: `scripts/devnet-deploy.sh harden 2`, then `ctl 2 restart cadre-header-relay`, wait for a relayed
   header, `ctl 2 restart cadre-node cadre-miner`, check heights agree (`status`).
2. box 1: the same.
3. box 0: `harden 0`, then restart one at a time with a check after each: cadre-attester, cadre-settle,
   cadre-relayer, cadre-crank, cadre-storefront, cadre-explorer, cadre-api, cadre-gateway, cadre-worker,
   cadre-swarm, cadre-web, then cadre-node + cadre-miner. Then `install 0` (or copy the Caddyfile) and
   `systemctl reload caddy` for the security headers. Undo: `rm -r /etc/systemd/system/<unit>.d/10-hardening.conf
   20-*.conf; systemctl daemon-reload; systemctl restart <unit>`.
Units added later (e.g. cadre-desktops, cadre-agents) get the same drop-in from `harden N`.

### Health check (box 0, LIVE since 2026-10-21 15:45 UTC)

`cadre-health.timer` runs scripts/devnet-health.sh (installed as /opt/lode/bin/cadre-health) every 5
minutes as lode: the three nodes answer, heights agree within 3 and the tip moved since the last run,
light-client final within 120 blocks of the chain (cadre-api /v1/summary), both relay payers hold at least
1 SOL, every enabled cadre-* unit on box 0 is active, at least 10% disk free, every certificate has 14+
days. Owner view: `scripts/devnet-deploy.sh ssh 0 cat /var/log/cadre-health/status` (one line: time,
OK/WARN/FAIL, failing checks) and `/var/log/cadre-health/health.log` (one line per check per run, last
20,000 lines). Thresholds can be overridden in /etc/lode/health.env. No external messaging (owner decision
needed for any alert channel). Boxes 1-2 are covered through their public RPC and the relay balances;
their unit states are not read from box 0.
When /opt/lode/bin/cadre exists the script also runs `cadre status --health --local --network devnet` and logs
its lines as `cli:<check>` (docs/CLI.md "status"; not installed yet: build it on box 0 and reinstall the script).
Section 9 (2026-10-23, after the 2026-10-22/23 outages; each shows within one or two 5-minute runs, looking
back `LOG_WINDOW` 10min in the journal): `desktop_tasks` (FAIL on any `model 402` in cadre-desktops or `vision_paused` on its /health, WARN when
half or more of at least 10 `task` events failed), `openrouter` (balance from
https://openrouter.ai/api/v1/credits with the key in the PROVIDERS_ENV_FILE named by /etc/lode/desktops.env,
sent as a header from stdin, never logged; WARN under $5, FAIL at $0 or below), `gateway_data` (WARN when
request_data bytes_waiting is over 16 GB or refused_storage / refused_node_storage rose since the last run),
`agents_inference` (WARN over 20 inference_hold + 402 lines from cadre-agents in the window),
`practice_thought` (WARN when cadre-practice logged no `thought` in 2 h). Tests:
scripts/test/devnet-health.test.sh. Thresholds: DESK_TASK_MIN, DESK_FAIL_RATIO, MIN_OPENROUTER_USD,
MAX_REQUEST_DATA_GB, AGENTS_PAY_MAX, PRACTICE_THOUGHT_MAX_AGE in /etc/lode/health.env.

### Backups

What matters, per box:
- keys `/etc/lode/keys/*`: box 0 ops, attester, storefront-wallet, miner, gateway, swarm/ (house agents);
  boxes 1-2 miner and relay. ops, attester, storefront-wallet and the node p2p seeds also exist on the Mac
  (keys/devnet, backup ~/.config/pearl-sol/keys-devnet-backup); the box-generated ones (relay.json on
  boxes 1-2, gateway.json, swarm/ and miner keys on box 0) exist ONLY on their box. Copying them off is the
  owner's call (offline medium, encrypted); this pass copied no key off any box.
- configs: /etc/lode (node.json carries the p2p seed), units and drop-ins, the Caddyfile.
- service state in /var/lib/lode: relayer.json (deposit/refund bookkeeping), storefront-state.json,
  gateway/ledger.json (prepaid tickets: money owed to users), cadre-api and explorer indexes (rebuildable),
  swarm/state.json.
- chain data /var/lib/lode/chain (2.7-3.6 GB): resyncs from the other nodes; a consistent copy needs the
  node stopped (window). Model files (1.2-1.7 GB) are re-uploadable from the test box copy.
`deploy/backup.sh` (root, on the box) writes /var/backups/cadre/cadre-<host>-<time>.tar.gz (0600, last 7
kept) with configs and state but no keys, no chain data, no models; it prints the number of key-like
entries (must be 0). Run once on boxes 0 and 2 on 2026-10-21 (box 0 1.7 MB, box 2 4 KB, 0 key-like
entries). Nothing is uploaded anywhere; where an offline copy lives is the owner's decision.

### Open (not fixed by this pass)

- box 0 `/var/lib/lode/gateway/requests` (inference request scratch, A and C files per request) has no
  pruning: 21 GB in 38 request dirs at 15:50 UTC, growing about 0.3 GB per minute while the house agents
  were calling it (21,571,180 KB to 21,896,828 KB in 60 s); box 0 had 93 GB free. Needs a retention rule in
  services/inference-gateway (owning rows); until then the health check's disk line is the alarm.
- caddy's admin API on 127.0.0.1:2019 is reachable by any local process (all services run as lode);
  moving it to a unix socket changes `systemctl reload caddy`, left as is.
- no rate limit at caddy in front of the node RPC (the node's own limits apply; chain review item).

## Contracts (2026-10-21, "smart contracts" row; docs/CONTRACTS.md)

Activation: devnet height 4700 (`DEVNET_CONTRACTS_HEIGHT`); the first contract tx landed in block 4703.
Nodes: agent-node 327d167b... from main bc4a6ef on boxes 1 (19:32 UTC), 2 (19:38), 0 (19:40), cadre-node only,
tips, finalized hash and peers equal after each (agent-node.pre-contracts kept); before that a fresh
follower on the test box replayed the whole devnet with the new binary (block 4587 hash c187d06a... and
state_root equal to all three boxes). Relays: header-relay 4bb4f670... (`--features devnet`; it keeps the
contracts record leaf in tx_root paths) on box 1, box 2 and box 0 cadre-settle, 19:05-19:10 UTC.
Explorer and cadre-api restarted on box 0 21:01 UTC (contract pages, /v1/contracts).

Example contracts, deployed and called with our own key (keys/devnet/contracts-keypair.json, funded with
200 CADRE from miner0), all with no admin (immutable):

| contract | address | created | calls made (block, gas, fee charged) |
|---|---|---|---|
| counter | DEgjVUmWddeiDduF2HpXR1z2rtXB7kLvyhvGb7YBAbgb | 4703 | increment 1 (4709, 150,670 gas, 4,368), increment 41 (4710); query get = 42 |
| escrow | ETyaKSLTxhsqZpqfX85rVTrM2Pt1B5j9svbiuxivXYng | 4705 | open with 1 CADRE for miner0 (4714, 249,617 gas, 5,947), release (4715, 290,180 gas, 5,743) |
| token "Cadre Demo Token" CDT | BfaxysW2erS1dPc37z4rsCQwobPrAknQ5A6q5o6P6aLB | 4707 | transfer 12,345 to miner0 (4711, 285,934 gas, 6,031); balance_of(miner0) = 12345 |
| bounty | 23ruAcWqrGY2VVLAVuAswNDGCJnrTeneZyBmPFDwpXb6 | 4713 | post with 2 CADRE and a town announcement as the contract (4717, 618,348 gas, 9,805), submit (4718), award (4719) |

Explorer: https://explorer.64-95-12-116.sslip.io/contracts and /contract/<address>; API:
https://api.64-95-12-116.sslip.io/v1/contracts. Screenshots: screenshots/explorer/devnet-*.png.
Use: `pearl --rpc https://rpc.64-95-12-22.sslip.io --key K contract call <address> <method> TYPE:VALUE ...`
(docs/CLI.md). `agent-node.pre-contracts` stays on each box but is no rollback target any more: blocks from 4703 carry
contract txs that it cannot decode.

## Files

deploy/provision.sh (root, fresh box), deploy/toolchain.sh (build box toolchains),
deploy/build.sh, deploy/test.sh, deploy/node-config.py, deploy/systemd/*.service,
deploy/Caddyfile*.tmpl, deploy/devnet.env (public facts), deploy/genesis.json, deploy/devnet-idl/;
scripts/devnet-deploy.sh (boxes), scripts/devnet-solana.sh + scripts/devnet-init/ (Solana side),
scripts/devnet-e2e.sh + scripts/devnet-e2e/ (end to end from the Mac).

## Trading (docs/TRADING.md, 2026-10-21)

Solana mandate program `BG5WkxZyYwWXTY2YSAJGhwL7JcxChRXx8UqHEAMAC5yc` (devnet build, 377,552 B,
max-len 415,307, upgrade authority the deployer; config `Euayzq1d...`, protocol 1,000 bps to the
storefront buyback vault). Cadre: AMM code `471b452b...`, mandate code `c39d2b15...`, DESK token
`GqHzh8yz...` and its CADRE pool `65QGmPmE...` (launched by the house trader desk_maker), mandate
contract `JCs8ZiUd...`. Addresses: deploy/trading-devnet.json, deploy/trading-devnet-solana.json,
deploy/trading-house.json. Keys: keys/devnet/trading/ (gitignored). House traders run on the test
box (`~/trading/run-trader.sh`; stop: `touch ~/trading/STOP; kill $(cat ~/trading/trader.pid)`);
the box 0 unit `deploy/systemd/cadre-trading.service` is ready for the orchestrator (TRADING.md 10).


## Blob lifecycle rollout (2026-10-22, HANDOFF item 37, SPEC 4.5 "Segment data retention")

Staged by the blob lifecycle row in `/home/lode/stage-bloblife/` (built on the test box from main a72713b):
agent-node `556739d39569ca62763eb6e36a395f232060fd920b85e8697f2573f9a0dc23a5` on boxes 0-2; cadre-gateway
`3827146388bad35547bc4236414bdb29acc753e603d78def77478e48afd668e0` and pearl-attester
`33c5b8977fae20ab36e4ae8e90a1ff9dfc1d7d0142413b78a6f5614b764032e0` on box 0. Running before: agent-node
5dcab264... (agent-node.dms), cadre-gateway 74098a4e..., pearl-attester fd5fc6d6.... The node build also carries the
Cx RPC fields (071e962, RPC only). Not a consensus change while nobody sets AdminConfig key 48 (do not set it before
all three nodes run this build and the tip is at 5800 or later), so old and new nodes mix and rollback is a swap.

Order: box 1, box 2, box 0, one at a time, as root on the box (`ssh -i ~/.ssh/lode_devnetN root@<ip>`).

Boxes 1 and 2 (mining-only: 16 GB quota, keep_replay false):

    rpc() { curl -s -XPOST 127.0.0.1:10630 -H content-type:application/json -d "{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"$1\",\"params\":{}}"; }
    rpc getChainInfo; rpc getBlobStats                       # baseline: height, finalized_hash, total_bytes
    sha256sum /home/lode/stage-bloblife/agent-node           # 556739d3...
    cp -p /etc/lode/node.json /etc/lode/node.json.pre-bloblife
    python3 -c 'import json; p="/etc/lode/node.json"; d=json.load(open(p)); d.setdefault("blobs",{}).update({"max_total_bytes":16000000000,"keep_replay":False}); open(p,"w").write(json.dumps(d,indent=1))'
    python3 -c 'import json; print(json.load(open("/etc/lode/node.json"))["blobs"])'
    cp -p /opt/lode/bin/agent-node /opt/lode/bin/agent-node.pre-bloblife
    install -m 755 /home/lode/stage-bloblife/agent-node /opt/lode/bin/agent-node.new && mv -f /opt/lode/bin/agent-node.new /opt/lode/bin/agent-node
    systemctl restart cadre-node
    until rpc getChainInfo | grep -q '"height"'; do sleep 2; done

Checks before the next box: `rpc getChainInfo` height and finalized_hash equal to the other two boxes within a block or
two, `rpc getPeers` count >= 2, `rpc getBlobStats` shows `retention.keep_replay` false, `max_total_bytes`
16000000000 and `classes`; `journalctl -u cadre-node -n 50` without errors; `systemctl is-active cadre-miner
cadre-header-relay`. After about 15 minutes (two prune passes): `classes.segment_a.bytes` falls toward the mining
tail (about 4.8 GB at 2026-10-22 traffic) and `fetching` stays well under 1024.

Box 0 (replay node: 40 GB quota, keep_replay default true), node first, then gateway, then attester:

    rpc getChainInfo; rpc getBlobStats; curl -s 127.0.0.1:10664/v1/stats
    sha256sum /home/lode/stage-bloblife/*
    cp -p /etc/lode/node.json /etc/lode/node.json.pre-bloblife    # the 30 GB file (an older backup: node.json.pre-blob30)
    python3 -c 'import json; p="/etc/lode/node.json"; d=json.load(open(p)); d["blobs"]["max_total_bytes"]=40000000000; open(p,"w").write(json.dumps(d,indent=1))'
    cp -p /opt/lode/bin/agent-node /opt/lode/bin/agent-node.pre-bloblife
    install -m 755 /home/lode/stage-bloblife/agent-node /opt/lode/bin/agent-node.new && mv -f /opt/lode/bin/agent-node.new /opt/lode/bin/agent-node
    systemctl restart cadre-node
    until rpc getChainInfo | grep -q '"height"'; do sleep 2; done
    # same checks as boxes 1-2, with retention.keep_replay true and max_total_bytes 40000000000
    cp -p /opt/lode/bin/cadre-gateway /opt/lode/bin/cadre-gateway.pre-bloblife
    install -m 755 /home/lode/stage-bloblife/cadre-gateway /opt/lode/bin/cadre-gateway.new && mv -f /opt/lode/bin/cadre-gateway.new /opt/lode/bin/cadre-gateway
    systemctl restart cadre-gateway
    until curl -sf 127.0.0.1:10664/health >/dev/null; do sleep 2; done
    curl -s 127.0.0.1:10664/v1/stats     # request_data has node_blob_headroom_bytes (after the first paid request) and refused_node_storage
    cp -p /opt/lode/bin/pearl-attester /opt/lode/bin/pearl-attester.pre-bloblife
    install -m 755 /home/lode/stage-bloblife/pearl-attester /opt/lode/bin/pearl-attester.new && mv -f /opt/lode/bin/pearl-attester.new /opt/lode/bin/pearl-attester
    systemctl restart cadre-attester
    systemctl is-active cadre-node cadre-gateway cadre-attester cadre-settle cadre-miner; journalctl -u cadre-attester -n 30

A restart of cadre-gateway interrupts a settle pass; the gateway resumes a half-posted round (it skips segments
already on chain) and a request in flight fails with its rows released. Watch 30 minutes: box 0 `df -h /` (over 20 GB
free), `getBlobStats` `total_bytes` and `classes`, the gateway's `request_data.bytes_waiting`, house thoughts in the town.

Rollback (per box, the same order reversed if needed):

    cp -p /opt/lode/bin/agent-node.pre-bloblife /opt/lode/bin/agent-node.new && mv -f /opt/lode/bin/agent-node.new /opt/lode/bin/agent-node
    cp -p /etc/lode/node.json.pre-bloblife /etc/lode/node.json && systemctl restart cadre-node
    # box 0 also: the same swap for cadre-gateway / pearl-attester (.pre-bloblife), restart each

After the drain (gateway `request_data.bytes_waiting` under 16 GiB), revert the stopgap:
`cp -p /root/inference.env.pre-drain /etc/lode/inference.env && systemctl restart cadre-gateway`.
deploy/devnet.env now carries the new quotas (BOX0_BLOBS 40 GB, BOX1/2_BLOBS 16 GB + keep_replay false), so a later
`devnet-deploy.sh` node.json write keeps them.

## Speed rollout (2026-10-22, HANDOFF item 39, docs/SPEED.md)

Staged by the chain speed row in `/home/lode/stage-speed/` on boxes 0-2 (built on the test box from branch speed
49c6700 (chain sources; later commits touch docs and tests only), which contains main up to 741194c): agent-node `8b4b3f749f2e6674149f95c073fc516129809915d470ae173419e6107fcb9222`, pearl-miner `9d78dda6d06cd717bab6097845eacf6109909fb43841fe7e26d6c20c08412068`.
Running before: agent-node 556739d3... (bloblife), pearl-miner 295bfca2....

What it changes: Phase 1 weak blocks and pre-confirmations (p2p and RPC only, inert toward old nodes) and the Phase 2
consensus change at **height 6000** (block 6000 migrates the config: 30 s target from 6001, K 60, emission_ppb
3500; uncles from 6000). Every node must run it BEFORE the tip reaches 6000 (about 18:40 UTC 2026-10-22 at 60 s
blocks): an old node rejects block 6000 (state_root mismatch) and forks off. Before 6000, old and new nodes mix and
rollback is a binary swap; after 6000 there is no rollback to the old binary (it cannot follow the migrated chain).
If the rollout cannot finish by about height 5980, stop and move the height (agent-types/src/speed.rs
DEVNET_SPEED_HEIGHT, rebuild, restage, and the light client set_speed height); never roll part of the boxes across 6000.

Order: box 1, box 2, box 0, one at a time, as root (`ssh -i ~/.ssh/lode_devnetN root@<ip>`):

    rpc() { curl -s -XPOST 127.0.0.1:10630 -H content-type:application/json -d "{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"$1\",\"params\":${2:-{\}}}"; }
    rpc getChainInfo                                          # baseline: height (must be < 5990), finalized_hash
    sha256sum /home/lode/stage-speed/agent-node /home/lode/stage-speed/pearl-miner   # 8b4b3f74... / 9d78dda6...
    cp -p /opt/lode/bin/agent-node /opt/lode/bin/agent-node.pre-speed
    cp -p /opt/lode/bin/pearl-miner /opt/lode/bin/pearl-miner.pre-speed
    install -m 755 /home/lode/stage-speed/agent-node /opt/lode/bin/agent-node.new && mv -f /opt/lode/bin/agent-node.new /opt/lode/bin/agent-node
    install -m 755 /home/lode/stage-speed/pearl-miner /opt/lode/bin/pearl-miner.new && mv -f /opt/lode/bin/pearl-miner.new /opt/lode/bin/pearl-miner
    systemctl restart cadre-node
    until rpc getChainInfo | grep -q '"height"'; do sleep 2; done
    systemctl restart cadre-miner

Checks before the next box: `rpc getChainInfo` shows `speed_height` 6000, height and finalized_hash equal to the
other two boxes within a block or two; `rpc getPeers` count >= 2; `rpc getParams` shows `speed_height`,
`speed_migration_height` 6000 and `emission_ppb` 0 (until 6000); `journalctl -u cadre-node -n 50` and
`journalctl -u cadre-miner -n 20` without errors (the miner's stats line has `weak_found` / `weak_accepted`, both
growing once this box's node runs the new build); `systemctl is-active cadre-node cadre-miner cadre-header-relay`.
After the first upgraded box: `rpc getWeakBlocks` on it lists weak blocks from its own miner; after all three, a
transfer shows `preconfirmed` on another box within a few seconds (`rpc getTxStatus '{"tx_hash":"<hash>"}'`).
Box 0 has the other services; only cadre-node and cadre-miner restart, nothing else.

Light client, once all three nodes run the build and before the light-client tip reaches 6000 (from the Mac; the
light-client admin is the ops key on box 0; the program was upgraded on 2026-10-21 (slot 513664592) and behaves as
before until this call; the running relays need no change):

    scp -i ~/.ssh/pearl_vps pearl@64.95.13.106:~/speed/stage/header-relay.speed /tmp/header-relay.speed
    scp -i ~/.ssh/lode_devnet0 -o IdentitiesOnly=yes /tmp/header-relay.speed lode@64.95.12.116:/home/lode/header-relay.speed
    ssh -i ~/.ssh/lode_devnet0 -o IdentitiesOnly=yes lode@64.95.12.116 '/opt/lode/bin/with-rpc /bin/sh -c "exec /home/lode/header-relay.speed --solana \$TOKEN_RPC --payer /etc/lode/keys/ops.json set-speed --height 6000 --target-ms 30000 --k 60"'
    ssh -i ~/.ssh/lode_devnet0 -o IdentitiesOnly=yes lode@64.95.12.116 '/opt/lode/bin/with-rpc /bin/sh -c "exec /home/lode/header-relay.speed --solana \$TOKEN_RPC --payer /etc/lode/keys/ops.json status"'   # "speed": {"speed_height": 6000, ...}

At 6000 and after (watch for an hour): every box `rpc getChainInfo` equal tips, finalized height = tip - 30 up to
5969 (tip 5999), then held at 5969 while K doubles (tip - 60 is lower) until the tip reaches 6030, then tip - 60; `rpc getParams` target_block_time_ms 30000, finality_k 60, emission_ppb 3500; block intervals drift from
about 60 s to 30 s over about an hour (WTEMA, half-life 1 h); header-relay `status` lag stays small and the light
client's final floor follows tip - 60; `rpc getBlock` of blocks after a depth-1 reorg shows `uncles`.

Rollback before 6000 (per box): `cp -p /opt/lode/bin/agent-node.pre-speed /opt/lode/bin/agent-node.new && mv -f
/opt/lode/bin/agent-node.new /opt/lode/bin/agent-node` (same for pearl-miner), restart cadre-node then cadre-miner.

## Job query perf rollout (2026-10-22, HANDOFF item 40)

Staged by the job query perf row in `/home/lode/stage-jobperf/` (built on the test box from main 64eeaaa, which
contains the speed and repair builds now running): agent-node
`6a194b232832fd6b9bee6c8e017a9bc230586066615db2728fbc98d2f48d1b23` (boxes 0-2), pearl-miner
`9daaf87117084f2626bf80e6738a003f93e464374ea0bc023cdb3ad536fb1416` and header-relay (devnet ids)
`562cfce6061e26577f38f9456d5e436de26d015b3d485d397e2a96cad34b9fbf` (box 0 only). Running before: agent-node
ee51b6de... (box 0), pearl-miner 9d78dda6..., header-relay 4bb4f670....

Node-local only (state read cache, job index, listJobs cursors, getNodeStats, template segment order, pool
eviction): no consensus change, no activation height, old and new nodes mix, rollback is a binary swap at any
height. Do NOT start before the height 6000 switch is verified ("Speed rollout": every box past about 6060,
`getParams` target_block_time_ms 30000 and finality_k 60, equal finalized hashes, finalized height moving again).

Box 1, box 2, then box 0, one at a time, as root (`ssh -i ~/.ssh/lode_devnetN -o IdentitiesOnly=yes root@<ip>`):

    rpc() { curl -s -XPOST 127.0.0.1:10630 -H content-type:application/json -d "{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"$1\",\"params\":${2:-{\}}}"; }
    rpc getChainInfo                                                  # baseline height, finalized_hash, peers
    sha256sum /home/lode/stage-jobperf/agent-node                     # 6a194b23...
    cp -p /opt/lode/bin/agent-node /opt/lode/bin/agent-node.pre-jobperf
    install -m 755 /home/lode/stage-jobperf/agent-node /opt/lode/bin/agent-node.new && mv -f /opt/lode/bin/agent-node.new /opt/lode/bin/agent-node
    systemctl restart cadre-node
    until rpc getChainInfo | grep -q '"height"'; do sleep 2; done
    rpc getNodeStats | head -c 300; echo                              # answers (the old build: unknown method)
    rpc listJobs '{"status":"open","kind":"job","limit":5}' | head -c 300; echo   # has "next_cursor"

Checks before the next box (give it 2-3 blocks): `rpc getChainInfo` height and finalized_hash equal to the other two
boxes within a block or two, peers >= 2; `journalctl -u cadre-node -n 50` shows "job index built" once (after the
first listJobs or auto template) and no errors; `systemctl is-active cadre-node cadre-miner`; on box 0 also
`systemctl is-active cadre-api cadre-explorer cadre-gateway cadre-settle cadre-relayer`. A node restart makes cadre-*
clients retry for a few seconds, as on every node restart.

Box 0 only, after its node, one at a time:

    sha256sum /home/lode/stage-jobperf/pearl-miner /home/lode/stage-jobperf/header-relay   # 9daaf871... / 562cfce6...
    cp -p /opt/lode/bin/pearl-miner /opt/lode/bin/pearl-miner.pre-jobperf
    install -m 755 /home/lode/stage-jobperf/pearl-miner /opt/lode/bin/pearl-miner.new && mv -f /opt/lode/bin/pearl-miner.new /opt/lode/bin/pearl-miner
    systemctl restart cadre-miner      # check: journalctl -u cadre-miner -n 20 (block_accepted / stats lines, no step_failed loop)
    cp -p /opt/lode/bin/header-relay /opt/lode/bin/header-relay.pre-jobperf
    install -m 755 /home/lode/stage-jobperf/header-relay /opt/lode/bin/header-relay.new && mv -f /opt/lode/bin/header-relay.new /opt/lode/bin/header-relay
    systemctl restart cadre-settle     # check: journalctl -u cadre-settle -n 20, no errors; only cadre-settle uses this binary on box 0
    # relayer (TypeScript, runs from /home/lode/pearl-sol): from the Mac `scripts/devnet-deploy.sh sync` (box 0 checkout to main), then on box 0:
    systemctl restart cadre-relayer    # check: curl -s https://relayer.64-95-12-116.sslip.io/health

After (box 0, root, read-only, 5 minutes; the before numbers are in HANDOFF 40 and TEST-REPORT):

    bash /home/lode/pearl-sol/scripts/devnet-rpc-load.sh 300

Expected: listJobs from pearl-miner about 1 call a minute (was 47), cadre-settle getJob about one per pending order
per block (was 223/min), relayer the same (was 112/min), node CPU and getChainInfo p90 down; getNodeStats shows the
cache hits and the per-method totals. Pool eviction shows as "dropped a pooled tx" in `journalctl -u cadre-node`.

Rollback (per box, any height): `cp -p /opt/lode/bin/agent-node.pre-jobperf /opt/lode/bin/agent-node.new && mv -f
/opt/lode/bin/agent-node.new /opt/lode/bin/agent-node`, restart cadre-node; box 0 the same for pearl-miner (restart
cadre-miner) and header-relay (restart cadre-settle); the relayer by checking out the previous commit in
/home/lode/pearl-sol and restarting cadre-relayer. The old pearl-miner and relayer work against the new node and the
new ones against the old node (offset fallback; per-tip cache only needs getChainInfo `tip`).

## Town on Solana (2026-10-22, HANDOFF item 42, docs/TOWN-ON-SOLANA.md)

LIVE (checked 2026-10-23: `/v1/ready` `checks.town_solana` ok, new `/v1/town` posts carry `chain:
solana`; `getParams` on all three nodes reports `town_sunset_height` 12000). The deploy steps below
are kept as the record. Program `town` devnet id `DAyyoBkSmyf3guaYpPp8FCAjRUPB5jZ9DCw6pMZCzGnZ` (key
`keys/devnet/town-keypair.json`, gitignored). Order: build with `--features devnet`, `scripts/devnet-solana.sh
deploy town`, `node scripts/town-init.ts ...` at once (anyone can init), fund the house operator
`Cu4bS99S37e2dsCfySse8NLkVr8goaCHmz1sRQPMVkMM` with SOL, then `TOWN_PROGRAM` in the box 0 environments of
cadre-api, cadre-desktops, cadre-swarm, cadre-agents and restarts one at a time; the site switch; then the
node rollout of the sunset gate before `DEVNET_TOWN_SUNSET_HEIGHT` (12000, provisional; tip 6143 at 20:17 UTC
2026-10-22). Exact commands: TOWN-ON-SOLANA.md section 7.

## Storefront pending orders (2026-10-23, HANDOFF item 51)

Read only diagnosis; nothing below has been run yet. One box at a time, health checks between.

**1. Header relay (unsticks the light client at 6300, then the 9 completed orders settle).** Build from main
with `--features devnet` (on box 0 as before, or the test box), keep the old binary:

    cd /home/lode/pearl-sol && sudo -u lode git pull --ff-only    # or scripts/devnet-deploy.sh sync from the Mac
    cd services/header-relay && sudo -u lode cargo build --release --features devnet --bin header-relay -j 2
    cp -p /opt/lode/bin/header-relay /opt/lode/bin/header-relay.pre-uncle
    install -m 755 target/release/header-relay /opt/lode/bin/header-relay.new && mv -f /opt/lode/bin/header-relay.new /opt/lode/bin/header-relay
    systemctl restart cadre-header-relay && journalctl -u cadre-header-relay -f    # expect "verified 6300", lag falling
    systemctl restart cadre-settle                                                  # same binary, settle side

Then boxes 1 and 2 (same binary, `systemctl restart cadre-header-relay`). Check `curl -s 127.0.0.1:10685/v1/summary`:
`light_client_lag.final_height` climbing past 6239, then `storefront.by_status.finalized` rising as the 9 closed
tickets (55, 58, 65, 74, 82, 83, 85, 91, 109) settle. Rollback: copy header-relay.pre-uncle back.

**2. storefront-x402 (no more exits after a payment; automatic reconcile).** After the code is on box 0's checkout:
`systemctl restart cadre-storefront`. New records are `v: 2`; `curl -s 127.0.0.1:10662/payments/unserved` lists what
is left. RECONCILE_MS (60000) / RECONCILE_AFTER_MS (120000) tune it.

**3. Refund the 7 legacy payments (176,900,335 lamports) and close the 6 drafts.** Storefront wallet
9kwgo5KN... held 1.247 SOL at 04:20 UTC. With the new code checked out:

    systemctl stop cadre-storefront
    cd /home/lode/pearl-sol/services/storefront-x402
    R() { sudo -u lode sh -c 'set -a; . /etc/lode/services.env; set +a; STOREFRONT_WALLET=/etc/lode/keys/storefront-wallet.json STATE_FILE=/var/lib/lode/storefront-state.json exec /opt/lode/bin/with-rpc /home/lode/.local/node/bin/node src/reconcile.ts "$@"' _ "$@"; }
    R                                   # lists 7 unserved + 6 drafts
    R --check '6PnAwZImo2K0qzN/DDlfBOe76w8c6+CTaS2mqZLravo4kSpKoNJweIZzB+MupGMRduKqBHfeT/372QeGd6T/BA==,HkdFg6EMAj+PLhwRsC0BrSaSuRLV+aN5Az2/4yZU03HVOCZOlr2lyxli/NdHipzEFBTBREgfYIuW/7stw7RuBg==,yM+vBZ7tg7r5dP133JCvo/aES968xoImzoc2cQbzdZzcFFxsKqJLjAjpJyrzHEd5zevrmV4E4LNuvVmVBjFPBg==,5ldAq9OTkWIadKJYhT5Q9yQjMHkdk7nuuH7yOYAXlaDYIt3xUiVEwEawCoBzMpzXsrGcy2xaFITnQBi52vfiBQ==,1wDpxfIiuwie8u+lDHCgPFQZ6KBeY+2oQGIYwTWoOSBGmdgYgKvHb2h01xiSAouQedsbhIHW9xEediTiBcQ0Ag==,Lms1M2r/sFuNs1sMlZqDRes/8Xu0bNzI/qj8Ru+Vd3f+L2zZeNwkJos2KvRzj6pk/yYLyCMQj2w+v0XCzjwxBQ==,GxZcBtd0WIia21/KFcqNImQuWQWcgSWAvsV6rG7QhRlSHKDv97bKOzZaMmiqnwB6ejkSgKfl2Q8uhxvylKlUCw=='                      # each must show unattributed_orders []
    R --refund '6PnAwZImo2K0qzN/DDlfBOe76w8c6+CTaS2mqZLravo4kSpKoNJweIZzB+MupGMRduKqBHfeT/372QeGd6T/BA==,HkdFg6EMAj+PLhwRsC0BrSaSuRLV+aN5Az2/4yZU03HVOCZOlr2lyxli/NdHipzEFBTBREgfYIuW/7stw7RuBg==,yM+vBZ7tg7r5dP133JCvo/aES968xoImzoc2cQbzdZzcFFxsKqJLjAjpJyrzHEd5zevrmV4E4LNuvVmVBjFPBg==,5ldAq9OTkWIadKJYhT5Q9yQjMHkdk7nuuH7yOYAXlaDYIt3xUiVEwEawCoBzMpzXsrGcy2xaFITnQBi52vfiBQ==,1wDpxfIiuwie8u+lDHCgPFQZ6KBeY+2oQGIYwTWoOSBGmdgYgKvHb2h01xiSAouQedsbhIHW9xEediTiBcQ0Ag==,Lms1M2r/sFuNs1sMlZqDRes/8Xu0bNzI/qj8Ru+Vd3f+L2zZeNwkJos2KvRzj6pk/yYLyCMQj2w+v0XCzjwxBQ==,GxZcBtd0WIia21/KFcqNImQuWQWcgSWAvsV6rG7QhRlSHKDv97bKOzZaMmiqnwB6ejkSgKfl2Q8uhxvylKlUCw=='
    R --cancel-drafts                   # 6 x 4,109,720 lamports back to the wallet
    systemctl start cadre-storefront

| paid (UTC) | payer | lamports | payment signature |
|---|---|---|---|
| 10-20 18:03:03 | 2RFcwRKuCHzFtnR7CMrMzmEDknNdqzoHWCMoW8FBzqWo | 25,114,840 | 5Sc7tYwMMcAZ2Tha5RCBdLqGNSaRA3nQFvY6182VhAZbyvkCizpKMB8oGPLyxfNZtSx6Uszh99Cocbb3FL99crsM |
| 10-21 16:06:29 | 9CDcU57w5vejND297HBjUvfxU58TuCHaaq7hJFwakaL7 | 25,152,550 | 28RkWT328ofNy78mWur5r5NM4pWfWt1z7A45CYrmTfa3hAU9PSKv1pxtArpefrJZChRSRbhy58AjmamcvrdpMN4S |
| 10-21 16:41:19 | Cu4bS99S37e2dsCfySse8NLkVr8goaCHmz1sRQPMVkMM | 25,442,615 | 5KT3LCfwX4E17454CZwqJ12UQpqgjQCWL6HMaCPrE2zpZNVW7ruurMPGMs4pszGNTqY4H4JRysWnSadgbmUWB9WH |
| 10-21 16:42:35 | Cu4bS99S37e2dsCfySse8NLkVr8goaCHmz1sRQPMVkMM | 25,442,615 | 4URHJe1UbDYQtd381m8b2X2qrFG77arChcPzwqZFXYYPpLn7kLMGi9iJVQQMbgrR1jrXMVj6sEDXPYfC89DSfnKY |
| 10-21 16:43:24 | Cu4bS99S37e2dsCfySse8NLkVr8goaCHmz1sRQPMVkMM | 25,442,615 | TZ8v8in3NGkjPhhgaVry5xEuLda4e3yud5XhxgFdJJiRLR4puDjcFgeqeWikLp9jGBnvqUZg2kVNyW8W5YxNLfn |
| 10-21 17:00:23 | FiL2yraRKYadA9KL8Po7PnP5Lh7tGsGEcr1h5gJgcge | 25,152,550 | YvLicCEyQPKKVz6hHGK2FwQTrtK7WFBGHqbAKVebmohigdBGSa5b63vvbk3ErmjNKTp8XBtZgPkPskiHjmMYsT5 |
| 10-21 17:01:34 | FiL2yraRKYadA9KL8Po7PnP5Lh7tGsGEcr1h5gJgcge | 25,152,550 | 41jYHucfAREVGhG8zcrUCVegQtgv4HTyAniTc1tQKunSarnpHu6Mep2vTf9qwxieeQPA2fQk6oT1vW2rqGwzEJGX |

Each payment tx is confirmed on devnet (payer down by exactly the amount, the wallet up by it less the fee),
no later wallet-signed transfer pays these payers except Wk3qXjme... (the refund of FiL2yr's third payment,
recorded), and no Order for these payers is unclaimed by another payment (`--check`, run read only from the Mac).
