# DefiLlama adapter for Cadre

`fees/cadre/index.ts` is a DefiLlama dimension adapter (fees and revenue) in the
DefiLlama/dimension-adapters layout: copy it to `fees/cadre/index.ts` there and its imports
(`../../adapters/types`, `../../helpers/chains`, `../../helpers/solana`) resolve to the real
modules. `adapters/` and `helpers/` here are local shims of only what it uses.

| DefiLlama field | Source (storefront program events, `Program data:` logs) |
|---|---|
| dailyFees, dailyUserFees | `OrderPlaced.sol_spent` + `BuybackShareHeld.lamports` of each buy_job tx in the window |
| dailyRevenue, dailyHoldersRevenue | `BuybackAccrued.lamports`: released buyback shares (settle_share on a finalized order) |
| dailySupplySideRevenue | `OrderPlaced.sol_spent`: buys the CADRE that pays the worker (W) and the mining pool (P) |

Amounts are SOL lamports added under the SOL mint; DefiLlama prices them. Shares of refunded or
expired orders go back to the buyer and never become revenue. The adapter walks the storefront
program's signatures over the window (`getSignaturesForAddress`, then `getTransaction`), which
needs an RPC with full history for the window. The mainnet storefront id goes in `STOREFRONT`
once deployed (today the localnet id; `CADRE_STOREFRONT_PROGRAM` overrides it).

    npm ci
    npm test                                             # unit: event layouts, window bounds, paging
    SOLANA_RPC=http://127.0.0.1:10600 npm run live -- --strict
        # all-time window on a running stack, cross-checked against the storefront Config
        # counters (total_sol, bb_accrued, every share ever held); --strict exits 1 on a mismatch

The explorer's `/revenue` page is the screened view (self-dealing excluded, unique payers);
DefiLlama gets the gross flows above, as its methodology expects.
