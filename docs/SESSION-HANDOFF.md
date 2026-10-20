# Session handoff (2026-10-20, ~04:45 UTC)

The owner is switching Claude accounts. Everything below is the state when the orchestrator
session (f2b59a9f) stopped all its agents. Read this first, then docs/HANDOFF.md (living state,
items 15-18), docs/DEVNET.md, docs/USE-CASES.md, and ~/pearl-sol/CLAUDE.md (rules, ports,
"Who is working on what", "Cadre web asks", "Cadre Wallet asks").

## Product
Cadre, ticker CADRE. Repo, crates, binary and the signed wire namespaces (`pearl-sol:...:v1`)
still say pearl; never rename the namespaces without new vectors.

## Repos (all private, all pushed)
| Repo | Branch | Local |
|---|---|---|
| github.com/plsdontcallmyfone/pearl-sol | main | ~/pearl-sol |
| github.com/plsdontcallmyfone/pearl-wallet | dev (release cadre-devnet-6.73.4) | ~/pearl-wallet |
| github.com/danishseal/cadre-extension | cadre-chain (same wallet code; teammate Chaitanya Pawar pushes here too; NEVER touch their main) | remote URL, not a configured remote |
| github.com/plsdontcallmyfone/cadre-web | main | ~/cadre-web |
| github.com/plsdontcallmyfone/cadre-docs | main | ~/cadre-docs |

## What is live
- Public devnet (3 BitLaunch boxes): chain https://rpc.64-95-12-116.sslip.io (also 64-95-13-213,
  64-95-12-22), explorer https://explorer.64-95-12-116.sslip.io, cadre-api
  https://api.64-95-12-116.sslip.io/v1. Solana devnet programs + CADRE mint
  BwJqbsJUXshRj9ABxXs4RZ838kZrSqw8TT4CKMNfEu5e (on PumpSwap). e2e run 2 passed 7/7 (run 1 failed
  step 4 on light-client lag). Since 2026-10-20: website https://cadre.64-95-12-116.sslip.io and paid AI
  (Llama-3.2-1B-Instruct on box 0, gateway through cadre-api /v1/openai); HANDOFF item 19.
- Test box 64.95.13.106 (key ~/.ssh/pearl_vps, user pearl): full demo stack LEFT UP (demo 11/11,
  Llama-3.2-1B-Instruct serving chat, prepaid, OpenAI API, receipts). cadre-api public at
  https://cadre-api.64-95-13-106.sslip.io. Stop with `scripts/demo.sh down` on the box.
- Nothing runs on the Mac for Cadre (site on 10710 and docs on 10675 were stopped).

## The two agents stopped mid-task (2026-10-20 ~04:40 UTC)
1. **Small fixes: pump-sell, keys list, evaluator index, wallet unlock bug.**
   - RESUMED AND DONE 2026-10-20 (wallet dev c2013368a, pushed to origin and danishseal cadre-chain):
     deploys verified live, keys list + evaluator index + sell pair checked on devnet / test box,
     unlock root cause fixed (double /unlock push on Lock; return by path now), 20/20 headless.
     The NOT DONE list below is history.
   - Landed and pushed in pearl-sol: cadre-gateway `GET /v1/keys` (a0d0546), explorer evaluator
     index `/api/jobs?evaluator=&status=` (e53d342), cadre-api `POST /v1/tx/pump-sell` (6d1e38c),
     docs SPEC 5.4 / API.md (1b36450). The new cadre-api answers on BOTH the test box and devnet
     box 0 (pump-sell route present on both), so it was deployed. Check that the test box
     cadre-gateway restart finished (`curl .../v1/openai/models` and a wallet-signed `GET /v1/keys`).
   - Wallet (~/pearl-wallet dev): CADRE -> SOL swap pair committed (fe55e9118) and pushed here.
     UNCOMMITTED and unfinished: docs/PEARL-PORT.md and regenerated screenshots (a UI run was in
     progress). The agent was adding mock routes for the UI check when stopped.
   - NOT DONE: keys page listing from the gateway, "Jobs I evaluate" reading the index, the
     unlock double-click root cause (first Continue after lock does nothing about half the time
     in headless runs; the UI check retries once), full wallet test run, push to cadre-chain.
2. **Light-client VerifyFailed on devnet.**
   - Root cause found and fixed in services/header-relay (de8edd5): the relay re-sent txs that
     had already landed; the "VerifyFailed" retries were BadStep 6015 / NotPending 6008. Relay
     parallelism cut to 3 slots per lane (32543c5); slots 3-9 closed and their rent returned to the
     relay payers.
   - NOT VERIFIED: that the fixed relay is running on devnet boxes 1 and 2, and the 60-minute watch
     (zero 6015/6008, small lag) was in progress. At 04:40 UTC the light client was 30 blocks
     behind the tip (final 2198, tip 2228), which is about the finality depth. Check with
     `journalctl` on boxes 1-2 (`scripts/devnet-deploy.sh ssh N`).
   - The ~7.3 SOL relay work-account rent: partly returned via the slot closes; confirm totals.
   - Uncommitted from this agent and committed by the orchestrator at handoff: crates/Cargo.lock
     (adds rand, sha2, ureq deps that were missing; fixes the installer's --locked fallback) and
     a one-line DEVNET.md update (3 slots per lane).

## Waiting on the owner
1. Dedicated Solana devnet RPC URL saved as one line in `~/pearl-sol/keys/devnet/solana-rpc-url`
   (0600, gitignored). Plumbing is deployed (`/opt/lode/bin/with-rpc`, falls back to the public
   RPC). Then: `scripts/devnet-deploy.sh rpc-url`, restart the Solana units on boxes 0-2, run
   `scripts/devnet-e2e.sh` twice (only one pass so far).
2. GPU for inference (a GPU provider key): the CPU box decodes about 0.4-0.6 tokens/s through
   the paid flow; devnet has no gateway. Also needed for the 7-8B int7 accuracy check.
3. Provisional values: reserve cap 100M (every buyback burns while the reserve holds more; devnet
   reserve holds 812M), fund_mining limits 10M/call, 3600 s, 20M/24h.
4. DONE 2026-10-20: website hosted on devnet box 0, wallet DEVNET preset has the site URL. New: the
   installer needs the miner source public (repo is private) for public users.
5. Whether cadre-chain becomes main on danishseal/cadre-extension.
6. Mainnet: prepared only (docs/MAINNET.md, section 11 lists the decisions); needs the owner's go-ahead.

## Other open items
- Wallet: no CADRE sell before fe55e9118 tested end to end; unlock double-click; keys list;
  evaluator index wiring (above).
- cadre-proxy not published to a registry.
- Stake-for-credit (holder perk, Venice DIEM model) planned, not built.
- No registration deposit for agents (spam possible); no shared routing to third-party model
  servers (SYNTHESIS item 9).
- Relayer and storefront-x402 tests crash on the boxes (known memory-allocation error); run them
  on the Mac.
- Cleanup: test box ~/agentlayer, ~/pearl-tok, /home/pearl/int7-exp (about 5 GB); Mac
  ~/pearl-sol/.demo (2.9 GB). ~/cadre-web has modified e2e screenshots from a test run, not committed.

## Standing rules (owner)
No em dashes anywhere; no monospace in UI (Geist); no invented numbers; headless Chromium only,
never the owner's Chrome; never publish claude.ai Artifacts; never generate logos; never print
keys (base58 in files); kill by PID only, never pkill -f node/solana/anchor; never change
`solana config` or ~/.config/solana/id.json (pass -k); never cargo clean before backing up
target/deploy/*-keypair.json; heavy builds on boxes; never touch instance-backend 64.94.84.126;
copy only the repo to boxes; devnet sends only with our own keys, never mainnet; commit only your
own files; add a "Who is working on what" row before writing.
