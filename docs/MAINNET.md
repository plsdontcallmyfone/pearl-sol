# Cadre mainnet: preparation only

Status (2026-10-20): PREPARED, NOT LAUNCHED. Owner decision 2026-10-20 (d): "mainnet: prepare only,
never launch". Nothing in this file has been run against Solana mainnet; no mainnet transaction was
sent while writing it (the only mainnet access was read-only `getAccountInfo` of pump.fun's Global and
FeeConfig, section 6). Launching needs the owner's explicit go-ahead and the decisions in section 11.

Everything below is the devnet procedure (docs/DEVNET.md, run end to end 2026-10-19/20) with the
differences mainnet brings. Every figure is either measured on the devnet (with its source) or read
from chain; where no measurement exists the text says so.

## 1. What is the same, what changes

| | devnet (live) | mainnet |
|---|---|---|
| Solana programs | reserve, bridge, storefront, lightclient, `--features devnet` ids | the same 4 programs, NEW program keypairs and a `mainnet` feature (not written yet, section 2) |
| pump.fun | real devnet programs (pump `6EF8...`, PumpSwap `pAMM...`, fees `pfee...`) | the same program ids on mainnet |
| pump Global | 0.3 SOL virtual reserves: a 1 SOL dev buy took the whole curve and it graduated at once | 30 SOL virtual reserves: the curve does not graduate on the dev buy (section 6) |
| CADRE mint | create_v2 on devnet, BwJqbsJU... | a new create_v2 on mainnet (Token-2022, 1B supply, 6 dp, mint and freeze authority null) |
| agent chain | 3 BitLaunch boxes, genesis deploy/genesis.json (chain_id 0x43414452) | a NEW genesis (new chain_id or at least new timestamp and keys); the devnet chain is never reused |
| Solana RPC | public api.devnet.solana.com (rate limited, the cause of the 2026-10-19 relay stall) | a keyed RPC is required (section 8) |
| storefront x402 network | `solana-devnet` | `solana` (mainnet) |

## 2. Code changes still needed before a mainnet build

None of these exist yet; each is small and follows the devnet pattern.

1. `token/programs/{reserve,bridge,storefront,lightclient}`: a `mainnet` cargo feature with
   `declare_id!` of the new mainnet program keypairs (today `#[cfg(feature = "devnet")]` /
   `#[cfg(not(feature = "devnet"))]`; storefront also its `DEFAULT_RESERVE_PROGRAM`).
2. `services/header-relay`: `--features mainnet` (it embeds the light-client id like `devnet`).
3. `deploy/mainnet.env` (box IPs, `SOLANA_RPC` placeholder only, program ids), `deploy/mainnet-idl/`
   from the build, `deploy/genesis.json` for the mainnet chain (new file, e.g. `deploy/mainnet-genesis.json`).
4. `crates/pearl-cli`: a `mainnet` network profile (endpoints, program ids).
5. `~/pearl-wallet` src/shared/pearl/networks.ts: a mainnet preset (node RPC, cadre-api, siteUrl, mint,
   program ids); `~/cadre-web` built with `NEXT_PUBLIC_CADRE_NETWORK=mainnet`.
6. `services/storefront-x402`: `X402_NETWORK=solana`; pricing env set by the owner (section 11).
7. `scripts/install-miner.sh`: `DEFAULT_RPC` of the mainnet node 0 (or a flag per network). The
   installer clones github.com/plsdontcallmyfone/pearl-sol, which is PRIVATE (GitHub answers 404 to
   an anonymous request, checked 2026-10-20): public miners cannot build until the repo (or a miner-only
   source tarball, or a prebuilt binary) is published. Same gap on devnet today.

## 3. Keys and who holds them

| key | role | where (devnet today) | mainnet plan |
|---|---|---|---|
| deployer | pays program deploys; upgrade authority of the 4 programs | Mac only (keys/devnet, backup ~/.config/pearl-sol/keys-devnet-backup) | Mac only, offline backup; upgrade authority moved to a multisig after launch checks (section 9) |
| program keypairs (4) + mint keypair | program ids, CADRE mint address | Mac only | generated fresh on the Mac, never copied to a box |
| ops | reserve admin, storefront admin, chain AdminConfig admin, cadre-settle fee payer, initializes everything | Mac + box 0 (/etc/lode/keys/ops.json) | split: an admin key that stays on the Mac (or the multisig) for admin instructions, and a separate settle payer on box 0 that only pays fees (finalize_order_proof needs the admin or an attester today, so this needs a small change or an attester key on the box) |
| attester (1 of 1) | chain attester, bridge signer, storefront attester | box 0 | the bridge and storefront run proof mode only, so the attester signs evaluations and refunds; 1 of 1 is a single point of failure: owner decides M of N (section 11) |
| storefront wallet | x402 payTo and fee payer of x402 txs | box 0 | box 0, funded with a small float only |
| relay payers | header-relay fee payers, one per relay box | generated on boxes 1 and 2 (`devnet-deploy.sh relay-key`) | same: generated on the box, never leaves it |
| miner payout keys | block rewards | one per box | same |
| node p2p keys | p2p identity | Mac, written into node.json on install | same |
| gateway key | inference gateway (ticket server, posts segments, chain fees) | generated on box 0 (`devnet-deploy.sh svc-key 0 gateway`) | same |
| dedicated RPC URL | carries the provider key | keys/devnet/solana-rpc-url (owner, not provided yet) | `keys/mainnet/solana-rpc-url`, copied by `rpc-url` (0600) |

## 4. Runbook (in this order; every step is a devnet step that already ran)

Preconditions: section 2 done and tested on devnet with the same commit; section 10 checklist green;
owner go-ahead in writing.

1. Boxes: buy 3 boxes (section 8), `deploy/provision.sh` (root) and `deploy/toolchain.sh` on the build
   box, ufw 22/80/443/p2p only (as devnet).
2. Chain: write the mainnet genesis (new chain_id/timestamp, admin = ops (or multisig-controlled key),
   attesters, `params {}` = SPEC defaults: 60 s blocks, K 30, emission 7 ppm, alpha 0.8). Genesis
   target: devnet started about 1000x too easy (DEVNET.md "Chain"); set it from the expected launch
   hash rate instead. `install N`, start node 0 then 1, 2, then the miners.
3. Build on the build box with `--features mainnet`: agent-node, pearl-miner, pearl-attester,
   header-relay, the 4 SBF programs, IDLs (deploy/build.sh).
4. Deploy the 4 programs from the Mac with the deployer (`devnet-solana.sh deploy` adapted: max-len
   1.1x the .so, buffers refunded), then `solana program show` each and compare the .so sha256.
5. reserve `initialize` with the bridge program id and the chain's mining pool (`getEscrow` pool).
6. Launch: `crank launch --name Cadre --symbol CADRE --uri <metadata URL> --devBuy <lamports>
   --mintKeypair <new mint key>`: pump.fun `create_v2` + the dev buy in ONE transaction, creator = the
   reserve PDA, tokens into the reserve. Dev buy size: section 6.
7. bridge `initialize` (mint, attester threshold, attesters).
8. storefront `initialize` (mint, bridge), the trader PDA's pump and PumpSwap volume accumulators,
   `sf-setup`, `sf-set --bps 1000 --dest reserve --maxPerCall 1000000000 --minBuy 1000000
   --minIntervalSecs 600` (the devnet values) and `--reserveCap 100000000000000` if not the default.
9. Light client: `header-relay init` on box 0 (checkpoint = the current final chain header), then
   `set_release_modes` / `set_finalize_modes` proof only (mode 2), as `devnet-init/init.ts` step 6.
10. Chain gas: ops buys a little CADRE and bridges it to ops and the delivering miner.
11. `fund_mining` from the reserve to the chain mining pool (permissionless, rate limited: section 5).
12. Start services: relayer, cadre-settle, storefront-x402, crank, explorer, cadre-api, cadre-web,
    header-relay on boxes 1 and 2 (one lane each), and the inference gateway + worker if a model is
    registered (devnet recipe: DEVNET.md "Inference").
13. Smoke: `scripts/devnet-e2e.sh` adapted to mainnet with small amounts (bridge in, x402 order, bridge
    out, buyback, refund), then the wallet and site checks.

## 5. Config values (devnet = provisional values the owner kept as final on 2026-10-20 (c))

| program / service | setting | value |
|---|---|---|
| reserve | `bb_reserve_cap` (storefront) | 100,000,000 CADRE: buybacks go to the reserve while it holds less, burned above |
| reserve | fund_mining limits | 10M CADRE per call, 3600 s between calls, 20M per 24 h; admin-editable (`crank fund-limits`), pausable |
| storefront | buyback | 1000 bps of order SOL, destination mining reserve, max 1 SOL per call, min buy 0.001 SOL, 600 s |
| storefront / bridge | finalize / release mode | proof only (light client) |
| bridge | attesters | 1 of 1 on devnet (owner decision for mainnet) |
| lightclient | params | mirror the chain AdminConfig at init (K 30, WTEMA, fallback m/n); any AdminConfig change must be mirrored with `set_params` |
| chain | params | SPEC defaults: 60 s blocks, K 30, emission 7 ppm, alpha 0.8, WTEMA half life 1 h, min_fee 1000, min_protocol_fee 0 |
| header-relay | slots | 3 per lane, 2 lanes (devnet since 2026-10-20) |
| storefront-x402 | pricing | PRICE_BASE_LAMPORTS 10,000,000, PRICE_LAMPORTS_PER_GFLOP 100,000, OVERHEAD_LAMPORTS 14,000,000 are code defaults, "not a market price": OPEN (section 11) |

## 6. pump.fun curve on mainnet vs devnet (read from chain, computed with pump's SDK)

Source: `node scripts/mainnet-prep/curve.cjs` (read-only `getAccountInfo` of pump's Global and FeeConfig
on both clusters, then `getBuyTokenAmountFromSolAmount` / `getBuySolAmountFromTokenAmount` from
@pump-fun/pump-sdk 4.1.0, the version services/crank uses), run 2026-10-20 at mainnet slot 459,116,111.

| Global field | mainnet | devnet |
|---|---|---|
| initial_virtual_sol_reserves | 30,000,000,000 (30 SOL) | 300,000,000 (0.3 SOL) |
| initial_virtual_token_reserves | 1,073,000,000.000000 | 1,073,000,000.000000 |
| initial_real_token_reserves (sold on the curve) | 793,100,000.000000 | 793,100,000.000000 |
| token_total_supply | 1,000,000,000.000000 | 1,000,000,000.000000 |
| pool_migration_fee | 15,000,001 lamports | 15,000,001 lamports |
| fees at launch (FeeConfig tier for a new curve) | protocol 95 bps + creator 30 bps | the same |

The curve is constant product on the virtual reserves: tokens_out = vT x sol_in / (vS + sol_in) on
the SOL left after fees. With 100x smaller virtual SOL, devnet prices every token 100x lower: the whole
curve (793.1M) costs 0.8607 SOL on devnet and 86.0679 SOL on mainnet. That is why the devnet dev buy of
1 SOL bought the entire curve (DEVNET.md: 822.37M CADRE into the reserve, migrated to PumpSwap two
seconds later) and why its storefront buybacks burn (the reserve is far above the 100M cap).

Dev buy on mainnet (create_v2 + buy in one tx, fees included):

| SOL in | CADRE to the reserve | % of supply |
|---|---|---|
| 0.3 | 10,493,887 | 1.05% |
| 1 | 34,199,203 | 3.42% |
| 2 | 66,285,714 | 6.63% |
| 3 | 96,449,438 | 9.64% |
| 5 | 151,660,777 | 15.17% |
| 10 | 265,758,514 | 26.58% |
| 20 | 426,004,963 | 42.60% |

| target reserve | SOL needed incl. fees |
|---|---|
| 1% (10M) | 0.2857 |
| 5% (50M) | 1.4846 |
| 10% (100M = the buyback reserve cap) | 3.1218 |
| 15% (150M) | 4.9363 |
| 20% (200M) | 6.9588 |

(research/NOTES-pump.md section 4 has the same table from 2026-10-16 and a LiteSVM check: 3 SOL bought
96,449,438 tokens, matching the row above.) What the size implies, with the kept limits: fund_mining
moves at most 20M per 24 h into the chain pool, so a 34.2M reserve (1 SOL) is drained into the pool in
about two days of maximum funding, a 100M reserve in five. Below 100M, storefront buybacks refill the
reserve instead of burning. Choosing the size is the owner's (section 11).

## 7. Cost estimate (measured devnet figures only)

Solana rent and fees are the same on mainnet and devnet (same rent rate, 5,000 lamports per
signature); priority fees are not (mainnet needs them under load; not measured here).

| item | SOL | source |
|---|---|---|
| program rent, 4 programs at max-len 1.1x the .so (programdata 424,837 / 438,072 / 686,998 / 718,124 B) | 11.5251 rent; 11.5387 measured with the deploy fees | `solana rent` of each programdata size; DEVNET.md "SOL spent" (deployer 89.9999 -> 67.9612 incl. 10 SOL to ops and 0.5 to the storefront wallet) |
| deploy buffers (held during the deploy, refunded) | 10.4775 | `solana rent` of each .so + 37 B (386,216 / 398,248 / 624,544 / 652,840 B) |
| one-time init by ops without the dev buy (configs, accumulators, light-client accounts, two 0.02 SOL gas buys) | about 0.16 (0.04 of it the gas buys) | DEVNET.md: ops 10 -> 8.84 after init including the 1 SOL dev buy |
| dev buy | 0.2857 to 86.07 | section 6 (owner picks) |
| header-relay work accounts (refundable, `close-works`) | 0.3544 each; 3 slots x 2 lanes = 2.1263 | `solana rent 69632` |
| header records (refundable on close) | 0.0042 per live header | SPEC 5.3 "Relay cost on real Solana" |
| relay fees per fallback header | 0.000185 base (37 signatures) + priority fees on about 12.2M CU | SPEC 5.3 (measured CU); 1,440 headers/day at 60 s = 0.27 SOL/day base |
| relay spend measured on devnet, no paid work (2026-10-20 05:33-06:18 UTC, 38 headers, both lanes) | 0.0140 net | relay payer balances XQUUCs5t... and E66ekRjf... (net of rent returned by closes) |
| relay spend measured on devnet, window with one inference round (06:18-08:53 UTC, 208 headers, 144 of them segment blocks whose jobs the relay registered with register_job) | 0.3544 net | same balances (4.66708 -> 4.47174 and 4.71261 -> 4.55359); if the no-paid-work rate above held for the other headers, the 144 segments cost about 0.28 SOL, about 0.0019 each (derived, rent of JobCommit accounts included) |
| one full e2e run on devnet (bridge in/out, x402 order, buyback, refund) | 0.0827 (ops) | DEVNET.md run 2: ops 17.9743 -> 17.8916 |
| one prepaid AI top-up of 256 rows (buyer) | 0.0954 | devnet paid flow 2026-10-20: 95,411,375 lamports (DEVNET.md "Inference") |
| boxes | $156/month | DEVNET.md "Boxes" (BitLaunch, 3 boxes) |
| keyed Solana RPC | not measured | owner picks a provider |

GPU inference (measured 2026-10-20 on one NVIDIA A40 48 GB, RunPod secure cloud on demand at
$0.59/hr; docs/measurements/gpu-a40-2026-10-20/, TEST-REPORT "GPU worker on RunPod"). Cost per 1M
output tokens = $0.59 / (measured tok/s x 3600) x 1e6; it counts the GPU only (no box, no chain fees,
no idle time), and the worker serves one request at a time per process.

| worker on the A40 | measured decode | $ per 1M output tokens |
|---|---|---|
| Llama-3.2-1B-Instruct w7a7, one worker process | 24.5-25.1 tok/s (8-1024 prompt tokens) | 6.52-6.68 |
| same, 8 worker processes on the one GPU (summed) | 84.9 tok/s (82.6 at 4, 78.1 at 16) | 1.93 |
| Llama-3.1-8B-Instruct w7a7, one worker process | 8.0-8.6 tok/s | 19.16-20.45 |
| 1B through the test box gateway (paid chat, 32 tokens, GPU worker over an ssh tunnel) | 21.13 tok/s decode; 8.7 s wall incl. ttft 1.53 s and 5.4 s copying segment files | 7.76 on decode alone |
| for comparison: the CPU worker on the 4-core test box, same chat | 0.376 tok/s; 110.6 s wall | (CPU box, not priced here) |

The whole RunPod session (one pod, 34 min up, bootstrap, 1B and 8B quantization, benches, checks,
accuracy evals, the paid chat) cost $0.33 by the account balance (50.00 -> 49.67).

Startup SOL on the deployer, from the rows above: 11.54 (programs) + 10.48 (buffers, returned) + the
dev buy + about 0.2 for init and gas + 2.13 per relay pair of lanes (returned on close) + a relay fee
float. Without the dev buy and before any refunds that is about 24.3 SOL, of which about 12.6 comes
back (buffers and work accounts).

## 8. Boxes, RPC, relay budget

Boxes (devnet, measured, DEVNET.md "Boxes"): box 0 nibble-8192 (4 vCPU, 8 GB, 150 GB) builds and runs
node 0, a delivering miner, attester, relayer, cadre-settle, storefront-x402, crank, explorer, cadre-api,
cadre-web and the inference worker + gateway; boxes 1-2 nibble-2048 (2 vCPU, 2 GB) run a node, a miner
and one header-relay lane each. BitLaunch Dallas, $104 + $26 + $26 = $156/month. Measured sizing:
agent-node RSS 458 MB at height ~200 (about 105 MB at 2,250 after restarts), the inference worker peaked
at 2.3 GB RSS (64 tokens, HANDOFF item 17), so box 0 is the only devnet box with room for it. For mainnet
the same three sizes work; more nodes from independent operators are a decentralization choice, not a
capacity need (owner).

Solana RPC: the public endpoints allow about 100 calls per 10 s per IP; on 2026-10-19 that stalled the
header relay for hours (DEVNET.md "Solana devnet RPC budget"). Mainnet needs a keyed RPC for every
Solana-using unit (relayer, cadre-settle, storefront-x402, crank, explorer, cadre-api, header-relay); the
plumbing exists (`/opt/lode/bin/with-rpc` reads `/etc/lode/solana-rpc-url`). No provider price was
measured here; the owner picks the provider and plan.

Relay budget (who pays): each header-relay lane pays its own Solana fees from its box key. Per-header
cost and the work-account rent are in section 7. Rewards: `relay_reward` from the light client's pool can
reimburse relayers (SPEC 5.3); funding that pool is an owner decision.

## 9. Upgrade authority plan

1. Deploy with the deployer as upgrade authority (as devnet), keep it on the Mac with an offline backup.
2. Launch checks (section 10) and the first days of operation with the deployer, so fixes can ship
   (devnet needed several program upgrades before it settled: DEVNET.md "Upgrade the Solana programs").
3. Then move the upgrade authority of all 4 programs to a multisig (`solana program set-upgrade-authority
   <id> --new-upgrade-authority <multisig vault>`, one per program), and the program admins (reserve,
   storefront, bridge, lightclient config admin = ops today) to the same multisig where the program
   allows `set_admin`-style changes. Making the programs immutable (`--final`) is possible later and
   cannot be undone.
4. The agent-chain AdminConfig admin (ops) follows the same plan; any chain AdminConfig change must be
   mirrored to the light client with `set_params`.

## 10. Pre-launch checklist

- [ ] Owner go-ahead in writing for mainnet, and every decision in section 11 made.
- [ ] Section 2 code changes merged, built with `--features mainnet`, and the SAME commit run on devnet
      end to end (scripts/devnet-e2e.sh 7/7 twice, scripts/devnet-inference.sh PASS).
- [ ] Test suites green on that commit (docs/TEST-REPORT.md list: chain, crates, LiteSVM for the 4
      programs, services, wallet, cadre-web).
- [ ] .so sha256 recorded before deploy and compared with `solana program dump` after.
- [ ] Deployer funded with the program rent plus the buffers (section 7) and the dev buy plus init.
- [ ] Mint keypair and the 4 program keypairs generated fresh, backed up offline, never on a box.
- [ ] Keyed Solana RPC URL in `keys/mainnet/solana-rpc-url` and installed on every box.
- [ ] Genesis written with the mainnet admin/attesters and a target from the expected launch hash rate.
- [ ] Token metadata URI served from a stable host (devnet uses the explorer's /cadre-token.json).
- [ ] Miner source or binary public (section 2 item 7), or the installer link stays off the site.
- [ ] Storefront pricing set (section 11), storefront wallet funded with a small float only.
- [ ] Monitoring: light-client lag, relay payer balances, node heights, unit health (devnet has
      `devnet-deploy.sh status`; nothing alerts yet).
- [ ] Rollback plan: pause fund_mining (`crank fund-limits --paused true`), storefront and bridge modes,
      and the upgrade path while the deployer still holds the authority.

## 11. Owner decisions still needed

1. Go / no-go and the launch date (owner 2026-10-20: prepare only).
2. Dev buy size (section 6): sets the starting reserve; 3.1218 SOL buys the 100M cap.
3. Attester set for mainnet: 1 of 1 (devnet) or M of N, and who holds the keys.
4. Multisig for the upgrade authority and admins, and when to hand over (section 9).
5. Keyed Solana RPC provider and plan.
6. Inference price (SPEC 4.5 OPEN DECISION) and the storefront knobs (PRICE_BASE_LAMPORTS,
   PRICE_LAMPORTS_PER_GFLOP, OVERHEAD_LAMPORTS); a model registration deposit (default none).
7. Whether to publish the miner source (or binaries) so the public installer works.
8. Relay reward pool funding, and whether third parties relay.
9. Mainnet genesis target and whether the same three boxes (or more operators) run the chain.
10. A GPU worker for inference: measured on an A40 (section 7: 25 tok/s per 1B worker process, 85 tok/s
    summed over 8 on one card, 8.0-8.6 tok/s for an 8B); which provider, how many cards, and whether
    the worker runs next to the gateway (the shim over ssh adds the segment-file copy to every request).
    The chain side for an 8B is DONE (2026-10-20, owner-approved): RegisterModel takes up to 1024
    matrices (SPEC 4.5), from genesis on a mainnet chain (devnet from height 4200).
11. Which model mainnet serves: keep Llama-3.2-1B-Instruct w7a7 (144 matrices, +2.51% PPL vs fp16) or
    register Llama-3.1-8B-Instruct w7a7 (640 matrices, +3.05% PPL vs fp16, research/INT7-ACCURACY.md 7),
    or both. Facts for the choice: the 8B runs at 8.0-8.6 tok/s per worker on an A40 against 25 tok/s
    for the 1B (section 7), so it needs a GPU worker (the CPU worker is not usable for it); its registered
    weights are 7,504,658,432 int8 entries (7.5 GB of B files, = its MACs per token row) that every miner
    serving it stores; each round posts 640 segments
    (10 PostSegments txs of 64) instead of 144, and on Solana each segment the light client registers is
    one register_job tx and one JobCommit account (SPEC 4.5), so 640 per round; the gateway's balance
    ticket budget is capped at 10,000 blocks (`max_blocks_balance`), which is 15 rounds of 640 segments
    against 64 rounds of 144 (budget 9,216 there) (raise the cap or segment fewer matrices per round before serving an 8B
    with prepaid balances). Devnet stays on the 1B and the CPU worker (owner, 2026-10-20).
