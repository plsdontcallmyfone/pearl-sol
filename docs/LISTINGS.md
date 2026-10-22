# Cadre data listings: DexScreener, CoinGecko, DefiLlama, bot feed (prep, 2026-10-22)

Status: prepared on the public devnet, ready to point at mainnet. Nothing here was submitted
anywhere: every listing form, every DefiLlama pull request and every bot account is the owner's
call (decision D9: external posting only with the owner's word per account).

Everything below is chain data. Figures quoted with a time were read from the live endpoint at
that time; read the endpoint for the current value.

## 1. Token info (what every listing asks for)

| Field | Value | Source |
|---|---|---|
| Name / symbol | Cadre / CADRE | pump.fun `create_v2` metadata (devnet: `https://explorer.64-95-12-116.sslip.io/cadre-token.json`) |
| Chain | Solana (SPL Token-2022 mint created by pump.fun) | token/, docs/MAINNET.md 4 |
| Decimals | 6 | the mint account (`/v1/supply` `decimals`) |
| Initial supply | 1,000,000,000 CADRE, fixed at create; no mint authority after create | pump bonding curve `token_total_supply` (`/v1/supply` `initial_supply_spl`) |
| Supply changes | only down: the storefront buyback burns CADRE above the reserve cap | SPEC 5.1 "Reserve cap"; `/v1/traderboard/burned` |
| Venue | pump.fun bonding curve, then PumpSwap after graduation | docs/MAINNET.md 6 |
| Devnet mint | `BwJqbsJUXshRj9ABxXs4RZ838kZrSqw8TT4CKMNfEu5e` | deploy/trading-devnet-solana.json |
| Devnet PumpSwap pool | `5vKCFRAKp4bxvGCxJL1S2vuTgjYEXoMHMQ8rnFGo5kfZ` | deploy/trading-devnet-solana.json |
| Mainnet mint / pool | not created yet (MAINNET.md runbook step 4) | |
| Agent chain | Cadre, its own useful-PoW chain; CADRE bridges 1:1 by lock/unlock (nothing minted there) | SPEC 1, 5.1 |

Description (plain words, no claims beyond the code): "Cadre is a chain where AI agents pay
for their own thinking. Every block is mined on a paid AI job. CADRE is the token on Solana
and bridges 1:1 to the Cadre chain; part of every paid order buys CADRE back into a mining
reserve and burns it above the reserve's cap."

Links to give a listing (devnet today; the mainnet hosts replace them):

| What | Devnet URL |
|---|---|
| Website | https://cadre.64-95-12-116.sslip.io |
| Explorer | https://explorer.64-95-12-116.sslip.io |
| Public API | https://api.64-95-12-116.sslip.io/v1 |
| Trader board (revenue, buyback, burn) | https://cadre.64-95-12-116.sslip.io/board, JSON `/v1/traderboard` |
| Leaderboards | https://cadre.64-95-12-116.sslip.io/leaderboards, JSON `/v1/leaderboards` |
| Source | https://github.com/plsdontcallmyfone/pearl-sol (public) |
| X, Telegram | none yet: the owner creates and approves each account |

## 2. Supply endpoints (CoinGecko and CoinMarketCap want plain numbers)

| Endpoint | Body | Read 2026-10-22 02:40 UTC (devnet) |
|---|---|---|
| `GET https://explorer.64-95-12-116.sslip.io/api/supply/circulating` | plain text, whole CADRE with 6 decimals | 146232092.620879 |
| `GET https://explorer.64-95-12-116.sslip.io/api/supply/total` | plain text, whole CADRE | 972262263.353381 |
| `GET https://explorer.64-95-12-116.sslip.io/api/supply` | JSON with every component and its account | |
| `GET https://api.64-95-12-116.sslip.io/v1/supply` | the same through cadre-api (`explorer.circulating_supply` in base units) | |

Definition (apps/explorer/src/supply.ts): total = live mint supply (1B minus burns);
circulating = total minus the mining reserve's token account minus the chain mining pool (chain
lamports / 1000) minus the storefront transit accounts. The bridge vault is not subtracted: its
tokens back the chain balances outside the escrow, which circulate there. Max supply = 1B
(nothing can mint).

## 3. DexScreener

DexScreener indexes pump.fun bonding curves and PumpSwap pools on Solana MAINNET by itself:
the pair page appears with the first trade, with no submission. On Solana devnet there is
nothing to list (DexScreener does not index devnet), so nothing was submitted.

What the owner may do on mainnet, after launch: the "Enhanced Token Info" form (token profile:
icon, header, description, links) is a paid order placed from the owner's account on
dexscreener.com. Use section 1 for the text and links. Rule kept: no generated logo or image;
the owner supplies any image. An agent never places that order.

## 4. CoinGecko (and CoinMarketCap)

CoinGecko lists a token after an application from the project team (coingecko.com "Request
Form"); it needs the token to trade on a DEX it tracks (PumpSwap qualifies on mainnet) and asks
for: contract address (the mainnet mint), chain, decimals, supply endpoints (section 2),
website, explorer (solscan for the mint, plus the Cadre explorer), source code, social links,
and a team contact. Prepared answers: section 1 and 2. Not possible on devnet; the owner submits
after the mainnet launch.

## 5. DefiLlama (fees, revenue, holders revenue)

`tools/defillama/fees/cadre/index.ts` is a dimension adapter in the DefiLlama/dimension-adapters
layout (tools/defillama/README.md). Fields, all decoded from program events on Solana:

| Field | Source |
|---|---|
| dailyFees, dailyUserFees | storefront `OrderPlaced.sol_spent` + `BuybackShareHeld.lamports`; mandate `Settled.fee_agent + fee_protocol` |
| dailyRevenue, dailyHoldersRevenue | storefront `BuybackAccrued.lamports` (released buyback shares); mandate `Settled.fee_protocol` (SOL into the same buyback vault) |
| dailySupplySideRevenue | `OrderPlaced.sol_spent` (buys the CADRE for the worker W and the mining pool P) + `Settled.fee_agent` |

Not counted: pump.fun creator fees the reserve crank spends on CADRE (DefiLlama's pump.fun
adapter already reports pump fees; adding them here would double count).

Checks (2026-10-22, test box, docs/TEST-REPORT.md "Ecosystem Phase 5"):
- `npm test`: event layouts, window bounds, paging, the mandate Settled offsets and fields;
- `CADRE_STOREFRONT_PROGRAM=9XjvGk2xJdByPL6NmUCQg7RdBLyeDjWjikZBnwuWvAKu CADRE_MANDATE_PROGRAM=BG5WkxZyYwWXTY2YSAJGhwL7JcxChRXx8UqHEAMAC5yc SOLANA_RPC=<devnet RPC with history> CADRE_API=https://api.64-95-12-116.sslip.io npm run live -- --strict`:
  the all-time window against the storefront Config counters and against cadre-api's own index
  (`/v1/buyback` accrued) and the Mandate accounts' `fees_protocol`. A public RPC prunes history,
  so the all-time check needs an RPC that keeps it; a mismatch names the pruned part.

Pointing it at mainnet: set `STOREFRONT` and `MANDATE` in the adapter to the mainnet program ids
(defaults in the file; env overrides stay for tests), set `start` to the launch date, rerun the
live check against a mainnet RPC with history, then open the pull request to
DefiLlama/dimension-adapters (the owner's GitHub account, owner's call).

## 6. Bot feed (decision D9)

The public event feed is live in cadre-api (docs/API.md "Boards"):
`GET /v1/events` (JSON, `since`, `types`, `organic=1`), `GET /v1/events.rss` (RSS 2.0),
`GET /v1/stream/events` (server-sent events). Types: buyback, burn, big_trade, hire_completed,
paid_dm_answered, new_agent, milestone; each links its tx. Thresholds are proposed defaults
(env `FEED_BIG_TRADE_LAMPORTS` 100000000 = 0.1 SOL, `FEED_BIG_BUYBACK_LAMPORTS` 0,
`FEED_MILESTONE_BLOCKS` 1000, `FEED_MILESTONE_PAID_BLOCKS` 100).

`services/feed-bot` posts that feed to a Telegram channel and/or an X account, and runs in DRY
RUN until all three are set: `FEED_LIVE=1`, `FEED_TARGETS=telegram` (and/or `x`), and the
target's credentials (`FEED_TELEGRAM_TOKEN_FILE` + `FEED_TELEGRAM_CHAT`, `FEED_X_TOKEN_FILE`,
0600 files, never printed). Hourly cap `FEED_MAX_PER_HOUR` (default 10); the first run takes no
backlog (`FEED_BACKFILL` to override). `node src/main.ts --status` shows the mode and why it is
dry. Unit: deploy/systemd/cadre-feed-bot.service (dry run as shipped; switching on is an
`/etc/lode/feed-bot.env` file the owner writes after approving the account).

## 7. Mainnet switch checklist

1. Mint and pool exist (MAINNET.md runbook); fill the mainnet rows of section 1.
2. Explorer and cadre-api on the mainnet hosts; supply endpoints answer (section 2).
3. DefiLlama: program ids + start date in the adapter, live check `--strict` passes, owner opens the PR.
4. DexScreener: the pair shows by itself after the first trade; the owner decides on the paid token profile.
5. CoinGecko / CoinMarketCap: owner submits with sections 1 and 2.
6. Bot: owner approves the Telegram channel and/or X account, writes `/etc/lode/feed-bot.env`, restarts the unit.
