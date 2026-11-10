# Readiness drills, 2026-11-10 (docs/READINESS-PLAN.md tracks B and D)

Run from the Mac against the rebuilt devnet (chain_id 1128350803, boxes 0 to 2), 11:00 to 12:30 UTC.
Solana side: still the public devnet RPC (429s), no keyed RPC; names: the sslip aliases.

## D2 per-client rate limits (Caddy)

Caddy 2.11.9 built with github.com/mholt/caddy-ratelimit on all three boxes (deploy/caddy-custom.sh,
`scripts/devnet-deploy.sh caddy N`; the apt binary stays at /usr/bin/caddy.default). Limits per client
address per minute: rpc 600, explorer 300, storefront 120, relayer 120. cadre-api (api, mcp) keeps
its own limit of 300 per minute. The admin API moved from 127.0.0.1:2019 to a unix socket
(O-caddy-admin closed).

| Test | Result |
|---|---|
| 700 getChainInfo in a burst at rpc2 | 570 x 200 (30 used just before), 130 x 429 with retry-after |
| 3000 getChainInfo, 40 parallel, rpc on box 0, 72 s | 1025 x 200, 1975 x 429; chain kept producing |
| 1500 GET /v1/summary, 40 parallel, api, 33 s | 279 x 200, 1221 x 429 (the app's own limiter) |
| Node RPC without the proxy, 4000 mixed getChainInfo/getBlock, 32 parallel | box 0: all 200, 4 ms average, 30 ms worst; box 2: all 200, 12 ms average, 48 ms worst (the client was the limit at 100 to 270 req/s). Box 0's miner fell from about 6,900 to 5,700 nonces/s while it ran |

## D3 ask box refill

`ask-setup.ts refill` + cadre-ask-refill.timer (every 10 minutes): one top-up of 8192 rows when the
demo balance is under 2048 rows, at most 6 a UTC day (about 0.15 devnet SOL). Proven on box 0: no-op
above the floor, exit 2 at the cap, one real top-up through the unit (order 230, 25,308,335 lamports).

## B6 kill and reboot drills (part 2; part 1 was 2026-11-09)

| Step | Recovery |
|---|---|
| Box 2: kill -9 node, miner, header relay | all three active and the node at the tip with 2 peers in 10 s |
| Box 2: reboot | node, miner, relay, caddy active, at the tip with 2 peers 29 s after the reboot command |
| Box 1: kill -9 node, miner, header relay | 11 s |
| Box 1: reboot | 28 s |
| Boxes 1 and 2 rebooted together | both back with 2 peers in 36 s; box 0's public RPC answered throughout |
| Box 0: kill -9 node and miner | at the tip with 2 peers in 15 s |
| Box 0: reboot | booted in about 95 s; rpc, api, explorer, storefront, mcp answering 122 s after the command; all 15 services active, none failed; a paid ask answered afterwards with a receipt |

No manual step anywhere. Seen on the way: no block for about 6 minutes around 11:10 UTC while the
two small miners were being restarted, after a stretch of fast blocks (12 blocks from box 0's miner
in 20 minutes against 41 in the 20 before). Block times are bursty; to look at in the soak (B10).

## B11 backup restore

The newest box 0 backup (2026-11-10 04:32 UTC, 80 MB, 2,787 entries) extracted clean into a scratch
directory. Scratch copies of cadre-api and the explorer started from the restored state on spare
ports: both loaded 15,632 blocks and 190 orders from the backup and reached the live tip (16,478)
within a minute, matching the live api's summary. relayer.json and storefront-state.json parse; the
restored relayer cursors equal the live ones.

Findings: (1) backups never left the box (the Mac pull job is still not approved); one copy was
pulled by hand to the Mac (~/.config/pearl-sol/devnet-backups, 0600). (2) A daily backup loses up to
a day of storefront payment records and relayer order state (restored 7 payments and 4 orders, live
9 and 5); the chain and Solana hold the truth, but the restore runbook must replay from them.
(3) Not done: a full rebuild on a separate scratch box (needs a box).

## B3 mempool flood (real funds, no minting)

New tool: `agent-node flood` (funds N fresh senders from one funded key by plain transfers, each
queues transfers at min_fee, then a probe at a higher fee). Run on box 2 against its own node, paid
from the box 2 miner's balance: 60 senders x 64 transfers.

| Measure | Result |
|---|---|
| Admission | 3,840 accepted, 0 refused, 2.5 s (about 1,500 tx/s on a 2-core box) |
| Blocks | 16495 carried 2,000 txs (the max_block_txs limit), 16496 the other 1,841; pool empty after two blocks |
| Probe at 10x min_fee, sent after the flood | included in the first block (16495) |
| Nodes | all three stayed at the tip with their peers |

Not reached: the pool's own cap (50,000 txs) and eviction; a flood that size needs about 800 senders.
A first try from the Mac through an ssh tunnel managed about 2 tx/s and stressed nothing.

## B4 inference concurrency (CPU worker, box 0)

N parallel chat completions (24 tokens each) through the gateway with the ask box key.

| Parallel | All answered | Slowest | Gateway request disk |
|---|---|---|---|
| 1 | 1 x 200 | 2.6 s | 864 -> 986 MB |
| 4 | 4 x 200 | 6.8 s | -> 1,473 MB |
| 8 | 8 x 200 | 14.6 s | -> 2,446 MB |
| 16 | 16 x 200 | 26.5 s | -> 4,394 MB |

Latency grows linearly (about 1.7 s per queued request); nothing was refused at 16 (queue limit 64).
Each request holds about 122 MB of scratch on disk until its ticket is final (K 60, 30 minutes or
more), so disk, not CPU, is the first limit: 110 GB free is roughly 900 requests inside one
finality window. The refusal above the cap was not reached and still needs a run at 64+.

## B5 blob store pressure (scaled, box 2)

A put of 4 MiB + 1 byte was refused (max_blob_bytes). 120 puts of 2 MiB offered in 6 s: 10 accepted
(a 20 MiB burst), 110 refused "rate limited"; the 625 model blobs stayed kept; the node stayed at
the tip. With the limits on the boxes (1 MiB/s, grace 100 blocks, max 16 GB) filling the store takes
hours and was not run; the open blob findings stay open (docs/READINESS-PLAN.md track C).

## B1 fresh node from genesis (the Mac, behind NAT, about 150 ms from the boxes)

Started 11:33 UTC with only the genesis file and the three seeds: 3 peers at once. Findings:
- Headers are fetched one at a time from the tip backwards, about 100 to 320 a minute from this
  distance: 16,500 headers take one to three hours before the first block is applied, and it grows
  with the chain. Needs batched header sync or checkpoints before mainnet.
- Nothing shows progress: getChainInfo says height 0, "syncing": false the whole time, and the node
  logs nothing at info level. I took it for a stall and restarted it after 31 minutes, which threw
  the fetched headers away (they are held in memory only).
- Result of the sync itself: see the progress log in docs/READINESS-PLAN.md.

## B12 wallet upgrade and mainnet flip (cadre-extension 493c836d2)

`scripts/cadre/flip-check.mjs`, headless Chromium, 55/55 from a real 0.1.0 build and 55/55 seeded:
accounts intact after the upgrade to 0.1.2; `mainnetLive: false` changes nothing; the live config
moves a devnet wallet to mainnet once; a hand switch back to devnet sticks; 14 bad documents (bad
signature, rollback, expired, forbidden fields, forged withdrawal) are refused; a withdrawal hides
mainnet and returns the wallet to devnet. No shipped code changed. Not covered: the Chrome Web
Store update path itself, and a withdrawal is silent for the user (publish it with a notice).

## C-U1 + C-U2 node patch rollout

Merged 9647be9 (anchored body scheduling with expiring asks; capped, expiring header fetch tracker;
10 new tests, chain workspace 276 passed). agent-node a51cf5f0... on box 2 first: its node was held
down for four blocks and caught up in 4 s. Then box 1 and box 0, 12:10 to 12:13 UTC; old binaries
kept as agent-node.prev.

## Found on the way

- `systemctl stop cadre-node` also stopped cadre-miner (Requires=) and starting the node did not
  bring the miner back. The unit now says Wants=; applied on all three boxes.
- Block times: averages over 100 blocks ranged from 9 s to 54 s in the last 12 hours against a 30 s
  target, with gaps of 4 to 6 minutes. Not caused by the drills; needs a look at the retarget.
- The Mac's disk filled during the builds (1.4 GB free at one point); build caches were cleared.
