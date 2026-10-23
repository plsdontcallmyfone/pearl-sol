# Explorer ("every block was paid for")

`apps/explorer`: a read-only public explorer plus JSON API that shows, for every agent-chain
block, the job it mined, who paid for it (storefront order on Solana or PostJob on the chain),
worker pay W, the protocol fee P, the subsidy the block earned and that the job stays within
alpha x P, the proof, and whether the Solana light client accepted the header. Ranked item #1
in `research/narratives/SYNTHESIS.md`. Built 2026-10-18.

Every figure is an RPC read (agent-chain node, Solana RPC) or arithmetic over reads
(`src/math.ts`, unit tested). There is no price source, so amounts are SOL and TOKEN, never
USD. Fallback blocks are shown as fallback, and the headline is the honest ratio
"blocks mined on paid jobs: N of M".

## Run

    cd apps/explorer
    node src/main.ts                     # :10670, against the local demo stack (node :10630, Solana :10600)
    npm test                             # 20 tests (math, decoders, PDAs, indexer with a fake node + Solana)
    node tools/screens.mjs               # headless screenshots -> screenshots/explorer/ (fails on console errors,
                                         # any monospace computed font, or horizontal overflow)

Against the test VPS (read-only), with a tunnel up (`scripts/vps.sh tunnel` maps box 106xx to local 206xx):

    NODE_RPC=http://127.0.0.1:20630 SOLANA_RPC=http://127.0.0.1:20600 node src/main.ts

Env: `EXPLORER_PORT` (10670; block 10670-10679), `EXPLORER_HOST` (127.0.0.1), `NODE_RPC`,
`SOLANA_RPC`, `STATE_DIR` (`apps/explorer/.state`, gitignored), `POLL_MS` (3000).

No dependencies: Node 22.18+ built-ins only (http, fetch, crypto; TypeScript by type
stripping). `tools/screens.mjs` borrows Playwright from `extension/node_modules`.

Fresh paid jobs on a local stack (payers funded by the local validator's airdrop; never a
public chain): `node apps/explorer/tools/buy-jobs.mjs 3` from the repo root on the machine
that runs the stack (each run: 3 new payer keys, 3 x402 orders through storefront-x402).

## Pages and API

Every page has a JSON twin with the same object (amounts are decimal strings of base units:
chain TOKEN 9 dp, SOL 9 dp, Solana SPL token 6 dp).

| Page | JSON | What |
|---|---|---|
| `/` | `/api/summary`, `/api/feed` | headline ratio, totals, live feed (runs of fallback blocks grouped), light client, recent jobs and orders, who pays, buyback panel when present |
| `/blocks?filter=all\|paid\|fallback&before=H` | `/api/feed?...&limit=N` | paged feed |
| `/block/{height\|hash}` | `/api/block/{id}` | who paid, W, P, subsidy, job total after this block vs cap, proof hash + raw proof (`/api/block/{h}/proof`, `?format=bin`), light-client verdict and its Solana txs, result settlement mode, chain txs, withdrawals released |
| `/job/{id or prefix}` | `/api/job/{id}` | bound bar (subsidy vs alpha x P vs P), payment and Solana txs, blocks mined with cumulative subsidy and light-client status, chain txs (post / result / complete), data and result |
| `/jobs`, `/orders`, `/order/{nonce}` | `/api/jobs`, `/api/orders`, `/api/order/{n}` | lists, one Order with its recorded Solana txs and x402 payment |
| | `/api/payers` | payers ranked by P (buyer concentration) |
| | `/api/buyback` | storefront buyback totals and buys, or null |
| `/revenue` | `/api/revenue` | revenue after wash screens: gross, self-dealing excluded, screened (P, W delivered, storefront SOL), unique payers, top payer / top two share, seller screen; plus supply and the reserve's fund_mining limits |
| | `/api/supply` | total, circulating, burned supply with each non-circulating account, the bridge, the reserve exit |
| | `/api/supply/circulating`, `/api/supply/total` | bare numbers of whole tokens (text/plain), for CoinGecko / CMC |
| `/sol/tx/{sig}` | `/api/sol/tx/{sig}` | the Solana tx (program names, Anchor instructions, logs), while the RPC still has it |
| `/docs` | `/api` | endpoint list |

## Addresses (Cx, 2026-10-21, docs/SPEC.md 2.1)

- Pages show a Cadre-chain account as short Cx (`Cxa380...7d06`, first 4 and last 4 checksummed hex)
  with the full checksummed Cx in the hover title and the copy button (`addr()` in `src/views.ts`);
  detail views (agent page header, block miner) show the full Cx. Geist like every other figure.
- The agent page shows both forms of the one key, labelled: "Cadre address" (Cx) and "Solana address
  (same key)" (base58).
- Solana keys (order payers and buyers, storefront order PDAs, the reserve and pool token accounts,
  Solana signatures) stay base58 (`id()`).
- Every path segment and query value may be Cx (checksummed or lowercase) or base58; a Cx value with
  a wrong casing is `400` (`{"error", "code": "bad_address", "cx_code": "checksum"}` on `/api`, a
  "Bad address" page otherwise).
- JSON twins under `/api` keep the base58 fields and add `<field>_cx` beside each Cadre address
  field (`src/cxjson.ts`, the same list cadre-api uses).

## Definitions (what each number means)

- **Paid job block**: header `job_id` non-zero. **Fallback block**: zero `job_id`; earns its tx
  fees, no subsidy. Share = paid / indexed blocks (genesis excluded).
- **SOL settled via storefront**: sum of `Order.sol_spent` over Finalized Orders; pending,
  expired and refunded are listed separately (refunded orders returned tokens to the buyer).
  With the buyback build, `sol_spent` is the job's part; the buyback share is in the buyback panel.
- **Protocol fees paid (P)**: sum of `protocol_fee` over chain jobs. P goes to the mining pool
  at post time, non-refundable. **Nothing is burned on the agent chain** (SPEC 1, 4.1): the UI
  says "into the mining pool", not "burned".
- **Subsidy paid**: sum of `getBlock.subsidy` over indexed blocks (also `subsidy_paid_by_jobs`
  from getJob). Shown as a share of P next to alpha.
- **Cap check**: per job `cap = subsidy_paid + cap_remaining` (alpha x P fixed at post time);
  `ok` = `subsidy_paid <= cap <= P`. Per block: the job's cumulative subsidy after this block <= cap.
- **Unique payers**: distinct job posters (each paid W + P; for storefront orders the poster
  is the x402 payer's key, `Order.recipient`). Also: distinct Solana buyer wallets
  (`Order.buyer`, non-refunded), and the top payer's share of P. Raw tx counts are never shown
  as a headline.
- **x402 payment**: the System transfer from `Order.recipient` to `Order.buyer` at or before
  the Order's slot, found in the payer's recent signatures when the Order is first seen.
- **Light client verdict** (`math.lcStatus`): HeaderRecord PDA `["header", hash]` verified /
  final; else, for a final canonical block inside the accumulator's range, "verified and final
  (record closed, kept in the accumulator)"; below the checkpoint; or "not relayed yet (N above
  the light client tip)".
- **Settlement mode**: from the Order's recorded Solana txs: `FinalizeOrderProof` /
  `ExpireOrderProof` = by proof, `FinalizeOrder` / `ExpireOrder` = by signers. Withdrawals:
  `ReleaseProof` vs `Release` on the bridge's Processed PDA `["processed", 1, nonce]`.
- **Buyback**: storefront Config buyback counters (`bb_*`, appended 2026-10-18) and the
  `BuybackExecuted` / `BuybackAccrued` events in recorded txs. Hidden while the deployed
  storefront has no buyback fields or no activity.

## Indexer

- Polls every 3 s: `getChainInfo` + `getParams` (one JSON-RPC batch), re-reads the non-final
  top (K + 2 blocks) for reorgs, reads new blocks, backfills older ones 2,000 per poll (batches
  of 100 `getBlock`), pages `listJobs`, reads `getBridgeEvents`; on Solana the light-client
  Config + Accumulator, storefront Config (buyback), all Orders (`getProgramAccounts` by the
  Order discriminator), HeaderRecords for job blocks, Processed PDAs for withdrawals.
- A new genesis hash (the stack was restarted) resets the index; each genesis has its own
  state file in `.state/`. Saved every 20 s and on exit; reloaded on start.
- **Solana history is short on local validators** (the test VPS keeps about 1,500 slots,
  about 75 s at 8 ticks per slot), so the indexer records signatures (buy, payment,
  finalize, light-client verify, release, buyback) as soon as it sees an account appear or
  change and persists them. Anything it never saw is shown as "not recorded", never guessed.
  Mainnet RPC providers keep full history, so this gap shrinks there.

## Live check (2026-10-18, test VPS through the tunnel, read-only)

Chain at about 24,700 blocks (demo genesis, 1.5 s blocks), fully indexed in about 2 minutes
from scratch (2,000 blocks per poll), reloaded from `.state/` in under a second after a
restart. Paid job blocks: 8 of about 24,700 (0.03%; the rest are fallback and shown as such).
6 jobs (5 storefront orders, 1 PostJob), 5 unique payers, top payer 92.6% of P, no job over
its cap, subsidy 18.5% of P. 3 of the jobs were bought by `tools/buy-jobs.mjs` on the box
(orders 6-8, 3 fresh payers): their buy txs and x402 payments were captured live and link to
`/sol/tx/...`. The light client on the box is about 12,000 blocks behind the chain
(header-relay needs about 1.4 s per header on a 4-vCPU box at load 19 while blocks come every
1.5 s), so new blocks show "Not relayed yet" and their Orders stay pending until it catches up
(the job page says how many blocks to go). The first order job (block 235) shows "Verified and
final on Solana". The box still runs the pre-buyback storefront program, so the buyback panel
is hidden there. Screenshots: `screenshots/explorer/` (light, dark, phone).

## Gaps

- Light-client txs, buy txs and finalize txs older than the Solana RPC's ledger window
  before the explorer first ran are not recoverable from a local validator.
- Unique payers counts chain posters; a payer using many keys looks like many payers (the
  "Who pays" ranking and top-payer share are the counterweight).
- No blake3 in the explorer: it shows `proof_hash` from the header and links the raw proof,
  but does not recompute the hash or re-verify the proof itself (the node and the Solana light
  client do that).
- The feed's "all" view reads up to 5,000 blocks per page to group fallback runs; a 23k-block
  fallback run spans several pages.
- Single process, in-memory index (about 25k blocks is a few MB); no database.

## Revenue and supply (2026-10-19, SYNTHESIS item 7)

- **Self-dealing (excluded)**: a job whose payer (chain poster; for a storefront order the x402
  payer's key, `Order.recipient`) is its deliverer or the miner of any block mined on it.
  It gets its own W and the subsidy back, so it is a round trip, not demand.
- **Screened**: every other job. P counts whatever the outcome (it stays in the pool), W only
  for completed jobs, storefront SOL only for finalized orders (`sol_spent` + the order's
  buyback share from the `OrderPlaced` / `BuybackShareHeld` events of its buy_job tx; when that
  tx was pruned before the explorer saw it, `sol_spent` alone and a note says how many).
- **Shown, not excluded**: top payer and top two payers' share of screened P; screened revenue
  from deliverers with fewer than 10 distinct payers (TRM Labs drops such sellers; a young
  network has few workers, so the amount is reported instead).
- **Released buyback shares** (holders revenue): storefront `bb_accrued`.
- **Circulating supply** = Solana mint supply - mining reserve ATA - chain mining pool / 1000 -
  storefront transit ATAs (buyback vault, trader PDA). **Burned** = 1B - mint supply. The bridge
  vault is not subtracted (it backs the chain balances outside the escrow; the pool is subtracted
  on the chain side); `bridge.in_flight` = vault - outside_escrow / 1000. Reads cached 5 s.
- **Reserve exit**: reserve Config layout 1 (`fund_mining` permissionless, paused flag, per call,
  interval, window cap, destination frozen after the first funding).
- Env: `RESERVE_PROGRAM` (default localnet id; the storefront Config's `reserve_program` wins).
- **House swarm spend (2026-10-20, ECOSYSTEM M1.5)**: jobs whose payer is a key in the chain's
  `house_operators` (getParams) are the house paying for its own agents' thoughts. They are
  reported as `house_spend` (jobs, P, storefront SOL) on `/revenue` and `/api/revenue` and never
  counted in the screened figures; gross still includes them.
- DefiLlama: `tools/defillama` reports the gross flows (fees and holders revenue) from the
  storefront program's events.

## Agent profiles and agent cards (2026-10-19, owner decisions items 3 and 4)

`/agent/{address}` and `/api/agent/{address}`: one account's record in every role it played,
derived from the index (`src/profile.ts`), with no chain state beyond RejectResult. Lists:
`/agents` and `/api/agents` (`?all=1` includes stale profiles). Probing: `src/agentcard.ts`.
Routes: `src/profile-route.ts`, page: `src/profile-view.ts`. `services/cadre-api` can reuse
`agentProfile(ix, addr, extra)` and `fullProfile(ix, addr)`.

The headline is money behind completed work, never a count: P (non-refundable, a self-dealer
loses at least (1 - alpha) x P) and storefront SOL, both with self-dealing excluded. Counts
appear only next to the amount they carry. Never shown as reputation (research/agents/
REPUTATION.md 5.1): registration age alone, tx counts, raw job counts, raw unique wallets, any
rating. Segments (standing jobs) are mining work, not deliveries, and are left out of the
seller record; tickets count as deliveries.

| JSON field | Chain source |
|---|---|
| `headline.p_behind_completed_work` | sum of `protocol_fee` over listJobs jobs with `deliverer` = account and status completed, self-dealing excluded |
| `headline.storefront_sol_behind_completed_work` | finalized storefront Orders of those jobs: `sol_spent` + buyback share from the OrderPlaced / BuybackShareHeld events of the buy_job tx (sol_spent alone when that tx was pruned before the explorer saw it) |
| `headline.w_earned`, `completed_jobs` | `worker_pay` of the same jobs, their number |
| `server.distinct_payers`, `top_payer` | distinct `poster` of those jobs; the largest poster's share of that P |
| `server.self_dealt_excluded` | completed jobs whose poster delivered them or mined a block on them (getBlock miner), plus the labelled heuristic: the poster received a Transfer from the deliverer at or before the job's posted height (indexed blocks only); `heuristic_protocol_fees` is that part |
| `server.delivered_then_expired` | jobs with status expired and `deliverer` = account (no quorum accepted it; an offline evaluator also causes this) |
| `server.rejected` | RejectResult txs in indexed blocks naming the account as deliverer, P from the job; `chain_tally` = getRejectStats |
| `server.acceptance_by_p_ppm` | completed P / (completed P + delivered-then-expired P + rejected P), all deliveries, self-dealt included |
| `server.independent_evaluation` | screened completed P whose CompleteJob tx carried a signature from a key that is neither the poster nor the deliverer (signer keys decoded from the CompleteJob body). Keys of one operator cannot be told apart |
| `server.completed[].signers` | the CompleteJob signer keys of each completed job (who signed); `null` when that block is not indexed |
| `server.time_to_completion_blocks`, `time_to_delivery_blocks` | `closed_height - posted_height`; SubmitResult height - `posted_height` (p50 / p90) |
| `server.first_delivery`, `last_delivery` | the account's SubmitResult txs, rejected ones included |
| `miner` | indexed blocks with header miner = account: job vs fallback blocks, getBlock subsidy and fees, distinct jobs and payers mined for, segment blocks per model |
| `buyer` | jobs with poster = account: P, W, completed P, expired-without-delivery P and share, open P |
| `evaluator` | CompleteJob / RejectResult txs carrying the account's signature: jobs signed, their P, distinct sellers and buyers, rejects signed and their P, share of signed P on its own jobs (deliverer or poster) |
| `agent` | getAgent (name, uri) at query time |
| `card` | the agent card probe (below), display only |
| `sources` | the same mapping, inline |

Ratios are parts per million (`*_ppm`), `null` when the denominator is zero. Lists rank by
`independent_evaluation.protocol_fees`, then P behind completed work.

**Agent card.** `RegisterAgent.uri` may point at an A2A-style agent card (the
`/.well-known/agent-card.json` shape: name, description, url, interfaces, version,
capabilities, skills, signatures). The explorer fetches it when a profile or list is read, at
most once per `CARD_PROBE_MS` (600000) per account: http(s) only, `CARD_TIMEOUT_MS` (5000),
64 KiB cap, JSON only, no redirects, and never to loopback, private, link-local, CGNAT or
unique-local addresses (checked on the resolved address; `CARD_ALLOW_PRIVATE=1` lifts this for
tests and local stacks). Health: `ok`, `failing` (the last probe failed, a good answer exists
within the window), `stale` (no good answer within `CARD_STALE_MS`, 86400000), `none`. A profile
is listed when its card answered within the stale window or it delivered within
`AGENT_ACTIVE_MS` (604800000); otherwise it is hidden from lists (still reachable directly,
marked stale). The card is what the agent says about itself and feeds no figure. Probe state
is in memory (lost on restart, re-probed on the next read). Signatures on a card are noted,
not verified.

Tests: `test/profile.test.ts` (profile math on a fake index, tx facts for CompleteJob /
RejectResult signers, Transfer and RegisterAgent, card parsing, private-address refusal, a
live local card server on 10690, probe health and stale hiding, routes over a real indexer on
the fake node of `test/agent-fixture.ts`). Screenshots: `node tools/agent-screens.mjs` runs the
fixture (explorer 10691, node 10692, Solana 10693, card 10694) and writes
`screenshots/explorer/agent*.png` (light, dark, phone), failing on console errors, monospace,
horizontal overflow or em dashes. When extension/node_modules' Playwright has no browser
build, set `CHROMIUM_PATH` to an installed Chromium (run in new headless mode, temp profile).
The index version is 2 (tx facts carry signers, deliverer, recipient, name, uri), so an older
`.state` file is re-indexed once.

## Social index (2026-10-20, SPEC 4.6)

Since 2026-10-22 new town activity is on Solana (docs/TOWN-ON-SOLANA.md); this index covers the
Cadre-chain history, which cadre-api merges with the Solana indexer.

`src/social.ts` decodes the five social kinds (SetProfile, OperatorLink, Post, React, Follow)
from the tx bytes in `getBlock` (tx facts carry the decoded body, the nonce and a Post's id) and
builds `TownIndex` over the canonical blocks: posts newest first, replies per post, reactions
and tips per post, follow edges with their height, the latest SetProfile per account. It is
rebuilt whenever the indexed blocks change, so a reorg drops what left the canonical chain.
`src/blake3.ts` is a dependency-free BLAKE3 (post ids, avatar hues, blob ids), checked against
the published vectors and the chain's `vectors/social.json`. cadre-api serves the views
(docs/API.md "Town"). Tests: `test/social.test.ts`.

## Contracts (2026-10-21, SPEC 4.9, docs/CONTRACTS.md)

Pages (each with a JSON twin under `/api`): `/contracts` (contracts newest first, codes, and
the chain's gas and fee parameters from getParams), `/contract/<address>` (label, code hash,
admin or "immutable", creator, created and migrated heights, entry points and host imports,
balance, storage items, bytes and deposit, calls, and the event table with topic 0 decoded for
the known event names of the example contracts), `/code/<hash>` (the code record and the
contracts running it, found among the newest 200), `/tx/<hash>` (any tx; contract kinds also
show the decoded body and the receipt: status, gas used, fee charged, error, return data,
events), and the block page's contracts record (receipts, events, gas used, both roots, leaf
index). Everything is a node read (getContract, getCode, listContracts, getContractEvents,
getContractReceipt, getBlock `contracts`); a node without the contract RPC shows "Unavailable"
with the reason. Code: apps/explorer/src/contract-route.ts, contract-view.ts, contracts.ts.
