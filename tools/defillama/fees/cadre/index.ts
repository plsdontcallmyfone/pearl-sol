// Cadre (CADRE): paid AI jobs on Solana through the storefront program, settled on Cadre's
// own useful-PoW chain. DefiLlama dimension adapter (fees + revenue), written in the
// DefiLlama/dimension-adapters layout: in that repo this file is fees/cadre/index.ts and
// the imports below resolve to the real modules.
//
// Every number is decoded from the storefront program's own Anchor events
// (`Program data:` logs), nothing is priced or estimated:
//   dailyFees / dailyUserFees  = SOL buyers paid into storefront orders in the window:
//                                OrderPlaced.sol_spent (the job's buy) + BuybackShareHeld.lamports
//                                (the order's buyback share), both from the buy_job tx
//   dailyHoldersRevenue        = released buyback shares (BuybackAccrued.lamports, settle_share on
//                                a finalized order): SOL the program spends buying back CADRE
//   dailyRevenue               = dailyHoldersRevenue (the protocol keeps no other margin)
//   dailySupplySideRevenue     = OrderPlaced.sol_spent: buys the CADRE that pays the worker (W)
//                                and the mining pool (P)
// A share of a refunded or expired order is paid back to the buyer (BuybackShareRefunded), so it
// is never holders revenue; it stays counted in fees for the day the buyer paid it.

import type { FetchOptions, SimpleAdapter } from "../../adapters/types";
import { CHAIN } from "../../helpers/chains";
import { getConnection } from "../../helpers/solana";
import { PublicKey } from "@solana/web3.js";
import { createHash } from "node:crypto";

// Mainnet program id goes here once the storefront is deployed there (localnet id by default;
// CADRE_STOREFRONT_PROGRAM overrides it, e.g. for the public devnet).
const STOREFRONT = process.env.CADRE_STOREFRONT_PROGRAM ?? "DxuuzbuTX32Na4yVDpjo7ZBZQWufFGjzGZASMRpPgXkr";
const SOL = "So11111111111111111111111111111111111111112"; // ADDRESSES.solana.SOL
const PAGE = 1000;

const disc = (name: string) => createHash("sha256").update(`event:${name}`).digest().subarray(0, 8);
const D_PLACED = disc("OrderPlaced");
const D_HELD = disc("BuybackShareHeld");
const D_ACCRUED = disc("BuybackAccrued");

export interface Flows {
  /** lamports */
  orderSol: bigint;
  shareHeld: bigint;
  shareReleased: bigint;
  orders: number;
}

/** Sums the storefront events in one tx's logs (Borsh layouts of token/programs/storefront). */
export function flowsFromLogs(logs: string[] | null | undefined, into: Flows) {
  for (const l of logs ?? []) {
    if (!l.startsWith("Program data: ")) continue;
    const d = Buffer.from(l.slice(14), "base64");
    if (d.length < 8) continue;
    const k = d.subarray(0, 8);
    // OrderPlaced { order 32, nonce 8, job_id 32, buyer 32, recipient 32, venue 1, sol_spent 8, ... }
    if (k.equals(D_PLACED) && d.length >= 153) {
      into.orderSol += d.readBigUInt64LE(145);
      into.orders++;
    }
    // BuybackShareHeld { order 32, nonce 8, lamports 8, ... }
    else if (k.equals(D_HELD) && d.length >= 56) into.shareHeld += d.readBigUInt64LE(48);
    // BuybackAccrued { order 32, nonce 8, lamports 8, ... }
    else if (k.equals(D_ACCRUED) && d.length >= 56) into.shareReleased += d.readBigUInt64LE(48);
  }
  return into;
}

/** Every successful storefront tx with blockTime in [start, end): newest first, paged by signature. */
async function storefrontFlows(start: number, end: number): Promise<Flows> {
  const conn = getConnection();
  const program = new PublicKey(STOREFRONT);
  const flows: Flows = { orderSol: 0n, shareHeld: 0n, shareReleased: 0n, orders: 0 };
  let before: string | undefined;
  for (;;) {
    const sigs = await conn.getSignaturesForAddress(program, { before, limit: PAGE }, "confirmed");
    if (sigs.length === 0) break;
    for (const s of sigs) {
      if (s.blockTime == null || s.err || s.blockTime >= end) continue;
      if (s.blockTime < start) return flows;
      const tx = await conn.getTransaction(s.signature, { commitment: "confirmed", maxSupportedTransactionVersion: 0 });
      flowsFromLogs(tx?.meta?.logMessages, flows);
    }
    before = sigs[sigs.length - 1].signature;
    if (sigs.length < PAGE) break;
  }
  return flows;
}

const fetch = async (options: FetchOptions) => {
  const f = await storefrontFlows(options.startTimestamp, options.endTimestamp);
  const dailyFees = options.createBalances();
  const dailySupplySideRevenue = options.createBalances();
  const dailyHoldersRevenue = options.createBalances();
  const dailyRevenue = options.createBalances();
  dailyFees.add(SOL, f.orderSol + f.shareHeld);
  dailySupplySideRevenue.add(SOL, f.orderSol);
  dailyHoldersRevenue.add(SOL, f.shareReleased);
  dailyRevenue.add(SOL, f.shareReleased);
  return { dailyFees, dailyUserFees: dailyFees, dailyRevenue, dailyHoldersRevenue, dailySupplySideRevenue };
};

const methodology = {
  Fees: "SOL buyers pay into Cadre storefront orders for AI jobs: the SOL that buys the job's CADRE (OrderPlaced.sol_spent) plus the order's buyback share (BuybackShareHeld).",
  UserFees: "Same as Fees: every order is paid by its buyer.",
  Revenue: "Released buyback shares (BuybackAccrued): the share of a delivered order's SOL that the storefront program spends buying back CADRE. Shares of refunded or expired orders go back to the buyer.",
  HoldersRevenue: "Same as Revenue: the buyback buys CADRE into the mining reserve up to its cap and burns the rest.",
  SupplySideRevenue: "OrderPlaced.sol_spent: buys the CADRE that pays the worker (W) and the mining pool (P).",
};

const adapter: SimpleAdapter = {
  version: 2,
  adapter: {
    [CHAIN.SOLANA]: {
      fetch,
      start: "2026-10-18",
    },
  },
  methodology,
};

export default adapter;
