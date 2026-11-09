# Cadre readiness plan (2026-11-09)

What is left to build, prove and stress before Cadre can be called finished on devnet and before a
mainnet launch is safe. Written after the 2026-11 rebuild; companion to docs/MAINNET.md (launch
runbook) and docs/REBUILD-2026-11.md (phase J). Every item says what "done" means. Sizes are working
time for one Claude session with the boxes available, not calendar time.

This repo is public: open security findings are named by id only; details stay in the local,
gitignored docs/security-private/.

## Where things stand

Proven since the rebuild: 3 nodes agree and mine (30 s blocks, K 60), public https endpoints, light
client current, contracts redeployed, house swarm posting, hosted agents service, ask route, daily
backups, wallet 0.1.1 (unit 380, UI 136/136, read-only live check 16/16).

Not proven since the rebuild: any paid flow end to end (bridge in and out past nonce 9, x402 order,
buyback, refund, paid inference with a receipt and settle on Solana), wallet write paths on the live
devnet, practice traders, desktops, a fresh node syncing from genesis, anything under load, any
failure drill. The last full e2e pass was on the old chain (2026-10-20).

Blocked on the owner today: DNS records, a Solana RPC key (both gate track A), website choice,
desktops go-ahead, launch chain.

## Track A: prove every function once (needs DNS + RPC key; about 1 day)

Run in this order; each failure is fixed before the next step.

| # | Check | Done when |
|---|---|---|
| A1 | `devnet-deploy.sh health` on the real names | every line OK except desktops |
| A2 | `scripts/devnet-e2e.sh` twice on one commit | 7/7 both runs: bridge in, x402 order, proof finalize, bridge out (first real withdrawal, nonce 9), buyback, refund |
| A3 | `INF_RUN=rebuild scripts/devnet-inference.sh` | answer returned, receipt, ticket completed, `finalize_order_proof` on Solana |
| A4 | Wallet write paths on a funded devnet account (headless): send both sides, bridge in and out, buy and sell CADRE, AI top-up, create and revoke an API key, register an agent, post, tip, follow, DM, market hire | each lands and shows final in Activity; screenshots retaken with live figures for the store |
| A5 | Site flows against the live API once the site is chosen: ask box, town, market, explorer links, connect wallet, miner installer on a clean machine (the repo is public now, so the installer can be tested for real) | no console errors, every link resolves, a fresh machine mines a block |
| A6 | Practice traders and one desktop (not thirty) | traders post receipts for a day; one desktop completes a task and shuts down cleanly |
| A7 | CLI (`cadre` rebuilt from main and installed on the Mac; the installed one is an old build) and MCP against the live names | CLI test list in TEST-REPORT green on the new chain |
| A8 | Clear the 60,000 CADRE transaction stuck at miner0 nonce 9 | mempool empty of it, miner0 can send |

## Track B: stress and failure drills (about 2 days, mostly waiting)

Nothing here has been done on the rebuilt chain. Each drill records numbers into
docs/measurements/ and a line in TEST-REPORT.

| # | Drill | What it answers | Done when |
|---|---|---|---|
| B1 | Fresh node from genesis on a 4th box (or the Mac) | can a stranger join; sync stalled twice in October | catches the tip, stays within 2 blocks for 6 h |
| B2 | Public RPC and API load: ramp read traffic on rpc and api names (k6 or a small script), per-client | where it bends; Caddy has no per-client limit today | rate limits in Caddy, block production unaffected at 10x today's traffic |
| B3 | Mempool flood at min_fee from funded test keys | fee market, eviction, block fullness, relay cost per block | blocks stay on time, honest txs at a higher fee land within 2 blocks |
| B4 | Inference concurrency on the CPU worker: 1, 4, 8, 16 parallel asks | backpressure and queue limits, disk held until final | refused cleanly above the cap, no disk growth past the cap |
| B5 | Blob store pressure: fill toward `max_total_bytes` | open findings C-U4, C-U5 in practice | node keeps serving, model blobs stay pinned |
| B6 | Kill and reboot drills: kill -9 each service, reboot each box in turn, both relay boxes at once | recovery without hands | every unit returns by itself, light client catches up, no manual step |
| B7 | Reorg and relay stall: partition one miner for 10 blocks, then heal | the light client stalled for 8 h on 2026-10-23 after a reorg | relay follows the reorg within 10 minutes, no stuck state |
| B8 | Starve drills: relay payer at 0 SOL, Solana RPC returning 429, box 0 disk at 95% | alerts fire, services degrade instead of corrupting | alert seen for each, clean recovery after refill |
| B9 | Bridge invariant under churn: 200 small bridge-ins and outs in random order | escrow equals minted supply at every step | invariant check green throughout |
| B10 | Soak: 72 h with traders, swarm, asks and the load script at 2x | leaks, drift, log growth | flat memory and disk slope explained, block time within 5% |
| B11 | Backup restore: rebuild box 0's services on a scratch box from the newest backup | backups are real | services start from the restored state |
| B12 | Wallet upgrade path: install 0.1.0-shaped storage, load 0.1.1, then flip a test build to a staging config with `launch.mainnetLive` | the flip in a real browser (only unit tested so far) | accounts intact, network moves once, withdraw falls back |

## Track C: security closure (re-checked 2026-11-09; about 1 week of work plus an outside audit)

The 18 findings left open on 2026-10-21 were re-checked against main on 2026-11-09 (details in the
local docs/security-private/RECHECK-2026-11-09.md). Result: 2 fixed since then (C-U6c job listing,
O-gw gateway disk), 2 partly mitigated (C-U3, C-U4), the rest open, and 2 new ones found (New-1,
New-2). The two wallet findings were fixed the same day in Cadre Wallet 0.1.2.

| Group | Ids | State | When it must land |
|---|---|---|---|
| Messages name the chain and deployment | C-U3, S-7, B-3, New-1, admin guards | open | BEFORE the mainnet genesis and first program deploy (signed layouts and consensus rules cannot change cheaply afterwards) |
| Initialize safety | B-4/L-6, R-3/S-4, B-3 launch window | open; the init scripts accept a program someone else initialized first | before any mainnet deploy: program gate + a launch script that checks after every step |
| Admin hygiene | A-1, S-8, admin powers | open | same program revision; multisig, threshold admin key and timelock are owner decisions |
| Buyback crank | R-4/S-5 | open, config never applied | owner decision (allowlist the cranker), before the first buyback |
| Chain sync flooding | C-U1 (High), C-U2 | open | node-local patch, no consensus change: can ship to devnet now |
| Blob store pressure | C-U4, C-U5, New-2 | partly | node-local patch + per-client limit at the proxy; registration deposit is free only at genesis |
| Node memory at restart | C-U6b | open | before the chain is large; not launch blocking |
| Light client reorg depth | residual | worse since K 60 | before mainnet (liveness only) |
| Ops | O-caddy-admin | open | with the next box config pass |
| Fixed | C-U6c, O-gw, wallet 1, wallet 2 | done | verify the gateway binary on box 0 is from main |
| Acceptable with a note | S-6, L-3 (while relay reward is 0), C-U6a, C-U6d, wallet 3 | open | documented |

Order: the three program groups are one program revision (one test pass, one devnet upgrade, then the
mainnet build). The chain message change is one consensus revision at the mainnet genesis. The two
node-local patches can roll on devnet first and double as tests for track B (B1, B5). Then the outside
audit of bridge, light client, reserve, storefront and the chain's consensus and sync paths; turn on
the gateway's strict audience mode once the store wallet is live.

## Track D: things still to build

| # | Item | Size | Needed for |
|---|---|---|---|
| D1 | Monitoring with alerts: health results to ntfy or Telegram, light-client lag, payer balances, disk, hosting runway (the Mac watcher still needs owner approval; a box-side watcher does not) | half a day | devnet finished, mainnet |
| D2 | Per-client rate limits on public rpc and api (Caddy) | 2 h | B2, mainnet |
| D3 | Ask box balance refill timer with a daily spend cap | 1 h | site launch |
| D4 | Deploy the chosen site with the ask box | 1 h after the owner confirms | devnet finished |
| D5 | Mainnet presets for site and CLI; network switch in the deploy scripts (wallet is done) | half a day | mainnet |
| D6 | `solana.cadre.ag` proxy (Caddy, rate limited, provider key server side) | 2 h | mainnet wallet flip |
| D7 | Mainnet e2e script with small amounts and a pause and rollback drill (`crank fund-limits --paused`, modes) | half a day | mainnet |
| D8 | GPU inference worker next to the gateway (batched worker measured at 140 tok/s on an A40; CPU is about 4 tok/s today) | 1 day plus the card | real users of AI |
| D9 | Dress rehearsal: the MAINNET.md runbook start to finish on fresh boxes against Solana devnet with mainnet-shaped keys, timed | 1 day | mainnet |
| D10 | Wallet font decision (Diatype licence) and store screenshots with live figures | 1 h | store upload |
| D11 | Robinhood Chain side (contracts, oracle, relayer, wallet EVM accounts) | 1 to 2 weeks | only if the owner picks RH |

## Order

1. Now, no owner input needed: C1 (re-verify findings), D1 box-side alerts, D2, D3, B1, B6, B11,
   B12, and B2 to B5 against the sslip names.
2. When DNS and the RPC key land: all of track A, then B7 to B10.
3. DONE 2026-11-09 in Cadre Wallet 0.1.2: the two wallet findings and the font (D10 screenshots with live figures still wait on track A).
4. Before any mainnet step: all of track C above the "acceptable" row, D5 to D9, the outside audit, then MAINNET.md section 10.

Devnet is "finished" when tracks A and B are green, D1 to D4 are live and the two node-local chain
patches are rolled. Mainnet additionally needs the rest of track C and D5 to D9.

## Progress log

- 2026-11-09: track C re-check done; wallet findings 1 and 2 and the font fixed (Cadre Wallet 0.1.2).
- 2026-11-09: found outside the plan: the light client had been stalled for 21 hours behind one
  timed-out send (header-relay kept a tx with no signature and never retried it). Fixed in 90debc6
  with two regression tests, rolled to boxes 0 to 2. This was drill B7/B8 happening by itself, and it
  shows D1 (alerts) is the most urgent build item: the health check flagged it all day to nobody.
- 2026-11-09: B6 part 1 (box 0, 22:00 UTC): kill -9 of 13 services by systemd MainPID (api, explorer,
  relayer, storefront, crank, attester, gateway, worker, settle, agents, swarm, agent-dump, web). All
  13 active again within 75 s with new pids; public endpoints 200, relayer health ok, a paid ask
  answered with the live block height. Not yet done: node and miner kills, box reboots, both relay
  boxes at once.
