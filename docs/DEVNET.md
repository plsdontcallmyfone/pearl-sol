# Cadre public devnet

The public devnet of Cadre (ticker CADRE): the agent chain on three of our own boxes, the token
side on Solana devnet. Everything here is devnet only: no mainnet, no pump.fun mainnet launch.
Owner decisions (2026-10-18): public devnet first; 3 new BitLaunch boxes, one node each; the
docs/SPEC.md chain parameters and buyback defaults unchanged; no domain, sslip.io names with https.

Status (2026-10-20): LIVE end to end, e2e run 2 7/7 PASS. Agent chain on 3 nodes (open p2p, miners on every
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
| cadre-api (public site API, docs/API.md) | https://api.64-95-12-116.sslip.io/v1 (`/v1/ready`, `/v1/summary`, ...): `cadre-api.service` on box 0, :10685 behind caddy; gateway, pool and prepaid off (no inference gateway on the devnet) |

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
| binaries | built on box 0 from commit fe8e04a (agent-node sha256 3a80b545...91f277d0, pearl-miner 356437dc...3b52ee0); was 424c2c1 until the 2026-10-19 rolling upgrade |

## Solana devnet programs (NEW keypairs, `--features devnet` builds)

| program | id | .so (sha256) |
|---|---|---|
| reserve | 8m1RBPiiscHWpXRDj4NevSFMN68tbgCC8f4hu6N4hVzi | 386,216 B, 13f78f26...613570c4 |
| bridge | 3S226LmVvsoA4QrEpwytiuLxREJrQHS7A1UkeDsJGFoN | 398,248 B, 1f6abc9c...2f7970e0 |
| storefront | 9XjvGk2xJdByPL6NmUCQg7RdBLyeDjWjikZBnwuWvAKu | 624,544 B, ad947c9a...2e717088 |
| lightclient | 47mavKipT2URcdj9qp2Stv6H5cM41cmzDXiTEcmDie4m | 652,840 B, e8786d59...5d58aef |

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
| 0 | lode-devnet-0 | 6ad580a0b64e983f5dbc913c | 64.95.12.116 | nibble-8192: 4 vCPU, 8 GB, 150 GB | build box; node 0, miner (2 threads, delivers), attester, relayer, header-relay, storefront-x402, crank, explorer, caddy | 0.156 | 104 |
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
cadre-storefront, cadre-crank, cadre-explorer, cadre-api (box 0); cadre-header-relay (boxes 1 and 2,
`HEADER_RELAY_BOX="1 2"`, lanes 0/2 and 1/2: each verifies the heights of its lane, submits and
connects are shared).

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
2. `scripts/devnet-deploy.sh sync` (git bundle to box 0, fast-forward), `scripts/devnet-deploy.sh build`
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
refund_deposit after 63.8 min; invariant OK after all 7 steps. Run 3 (the second passing run) waits
for the dedicated RPC below.

### Dedicated Solana RPC (owner decision 2026-10-20)

The devnet services use a dedicated RPC from keys/devnet/solana-rpc-url (one line, gitignored, 0600,
written by the owner) once it exists. `scripts/devnet-deploy.sh rpc-url` copies that file to
/etc/lode/solana-rpc-url on each box (0600, lode; the only file besides the repo that goes to the
boxes), then `ctl N restart` the units. Every Solana-using unit (relayer, storefront, crank, explorer,
cadre-api, cadre-settle, cadre-header-relay) starts through /opt/lode/bin/with-rpc (deploy/with-rpc.sh),
which reads the file at start; without it the public RPC in the env files stays. Logs show only
scheme://host of the Solana RPC. The Mac-side scripts (devnet-solana.sh, devnet-init, devnet-e2e) keep
the public RPC. Status 2026-10-20 03:50 UTC: plumbing deployed, file not provided yet, so every
service runs on the public RPC.

SOL totals (devnet, 2026-10-20 03:50 UTC): deployer 89.9999 -> 45.9612 (11.54 program rent, 20 to ops,
0.5 to the storefront wallet, 12 to the two relay keys); ops 17.8916, storefront wallet 0.5190, relay
box 1 2.0896, relay box 2 2.2225; 20 relay work accounts hold about 7.3 SOL rent (refundable with
`header-relay close-works`).

## Files

deploy/provision.sh (root, fresh box), deploy/toolchain.sh (build box toolchains),
deploy/build.sh, deploy/test.sh, deploy/node-config.py, deploy/systemd/*.service,
deploy/Caddyfile*.tmpl, deploy/devnet.env (public facts), deploy/genesis.json, deploy/devnet-idl/;
scripts/devnet-deploy.sh (boxes), scripts/devnet-solana.sh + scripts/devnet-init/ (Solana side),
scripts/devnet-e2e.sh + scripts/devnet-e2e/ (end to end from the Mac).
