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
| `/sol/tx/{sig}` | `/api/sol/tx/{sig}` | the Solana tx (program names, Anchor instructions, logs), while the RPC still has it |
| `/docs` | `/api` | endpoint list |

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
