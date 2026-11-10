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
