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
//
// Delegated trading (mandate program, docs/TRADING.md), when CADRE_MANDATE_PROGRAM / MANDATE is set:
//   dailyFees += Settled.fee_agent + Settled.fee_protocol   (performance fees users' trading accounts paid)
//   dailySupplySideRevenue += Settled.fee_agent              (to the trading agent)
//   dailyRevenue, dailyHoldersRevenue += Settled.fee_protocol (plain SOL into the storefront buyback
//                                                              vault, which the crank spends on CADRE)
// Not counted here: pump.fun creator fees the reserve crank spends on CADRE. DefiLlama's pump.fun
// adapter already reports pump fees, so adding them again would double count.

import type { FetchOptions, SimpleAdapter } from "../../adapters/types";
import { CHAIN } from "../../helpers/chains";
import { getConnection } from "../../helpers/solana";
import { PublicKey } from "@solana/web3.js";
import { createHash } from "node:crypto";

// Mainnet program id goes here once the storefront is deployed there (localnet id by default;
// CADRE_STOREFRONT_PROGRAM overrides it, e.g. for the public devnet).
const STOREFRONT = process.env.CADRE_STOREFRONT_PROGRAM ?? "DxuuzbuTX32Na4yVDpjo7ZBZQWufFGjzGZASMRpPgXkr";
// Mandate program (delegated trading). Empty = not deployed on this cluster: no mandate flows.
// Public devnet: BG5WkxZyYwWXTY2YSAJGhwL7JcxChRXx8UqHEAMAC5yc (deploy/trading-devnet-solana.json).
const MANDATE = process.env.CADRE_MANDATE_PROGRAM ?? "";
const SOL = "So11111111111111111111111111111111111111112"; // ADDRESSES.solana.SOL
const PAGE = 1000;

const disc = (name: string) => createHash("sha256").update(`event:${name}`).digest().subarray(0, 8);
const D_PLACED = disc("OrderPlaced");
const D_HELD = disc("BuybackShareHeld");
const D_ACCRUED = disc("BuybackAccrued");
const D_SETTLED = disc("Settled");

export interface Flows {
  /** lamports */
  orderSol: bigint;
  shareHeld: bigint;
  shareReleased: bigint;
  orders: number;
  /** mandate Settled events: the agent's and the protocol's fee, lamports */
  mandateAgentFee?: bigint;
  mandateProtocolFee?: bigint;
  settles?: number;
}

/** Sums the mandate program's Settled events in one tx's logs (Borsh layout of token/programs/mandate). */
export function mandateFlowsFromLogs(logs: string[] | null | undefined, into: Flows) {
  for (const l of logs ?? []) {
    if (!l.startsWith("Program data: ")) continue;
    const d = Buffer.from(l.slice(14), "base64");
    // Settled { mandate 32, pnl i64, hwm i64, fee_agent u64, fee_protocol u64, equity_after u64 }
    if (d.length >= 80 && d.subarray(0, 8).equals(D_SETTLED)) {
      into.mandateAgentFee = (into.mandateAgentFee ?? 0n) + d.readBigUInt64LE(56);
      into.mandateProtocolFee = (into.mandateProtocolFee ?? 0n) + d.readBigUInt64LE(64);
      into.settles = (into.settles ?? 0) + 1;
    }
  }
  return into;
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

/** Every successful tx of `programId` with blockTime in [start, end): newest first, paged by signature. */
async function programFlows(programId: string, start: number, end: number, flows: Flows, decode: (logs: string[] | null | undefined, into: Flows) => Flows): Promise<Flows> {
  const conn = getConnection();
  const program = new PublicKey(programId);
  let before: string | undefined;
  for (;;) {
    const sigs = await conn.getSignaturesForAddress(program, { before, limit: PAGE }, "confirmed");
    if (sigs.length === 0) break;
    for (const s of sigs) {
      if (s.blockTime == null || s.err || s.blockTime >= end) continue;
      if (s.blockTime < start) return flows;
      const tx = await conn.getTransaction(s.signature, { commitment: "confirmed", maxSupportedTransactionVersion: 0 });
      decode(tx?.meta?.logMessages, flows);
    }
    before = sigs[sigs.length - 1].signature;
    if (sigs.length < PAGE) break;
  }
  return flows;
}

export async function flowsInWindow(start: number, end: number): Promise<Flows> {
  const f: Flows = { orderSol: 0n, shareHeld: 0n, shareReleased: 0n, orders: 0, mandateAgentFee: 0n, mandateProtocolFee: 0n, settles: 0 };
  await programFlows(STOREFRONT, start, end, f, flowsFromLogs);
  if (MANDATE) await programFlows(MANDATE, start, end, f, mandateFlowsFromLogs);
  return f;
}

const fetch = async (options: FetchOptions) => {
  const f = await flowsInWindow(options.startTimestamp, options.endTimestamp);
  const agent = f.mandateAgentFee ?? 0n, proto = f.mandateProtocolFee ?? 0n;
  const dailyFees = options.createBalances();
  const dailySupplySideRevenue = options.createBalances();
  const dailyHoldersRevenue = options.createBalances();
  const dailyRevenue = options.createBalances();
  dailyFees.add(SOL, f.orderSol + f.shareHeld + agent + proto);
  dailySupplySideRevenue.add(SOL, f.orderSol + agent);
  dailyHoldersRevenue.add(SOL, f.shareReleased + proto);
  dailyRevenue.add(SOL, f.shareReleased + proto);
  return { dailyFees, dailyUserFees: dailyFees, dailyRevenue, dailyHoldersRevenue, dailySupplySideRevenue };
};

const methodology = {
  Fees: "SOL buyers pay into Cadre storefront orders for AI jobs: the SOL that buys the job's CADRE (OrderPlaced.sol_spent) plus the order's buyback share (BuybackShareHeld); plus the performance fees users' delegated trading accounts pay (mandate Settled fee_agent + fee_protocol).",
  UserFees: "Same as Fees: every order and every trading fee is paid by its user.",
  Revenue: "Released buyback shares (BuybackAccrued): the share of a delivered order's SOL that the storefront program spends buying back CADRE, plus the protocol share of trading fees (Settled.fee_protocol), sent as SOL to the same buyback vault. Shares of refunded or expired orders go back to the buyer.",
  HoldersRevenue: "Same as Revenue: the buyback buys CADRE into the mining reserve up to its cap and burns the rest.",
  SupplySideRevenue: "OrderPlaced.sol_spent (buys the CADRE that pays the worker W and the mining pool P) plus the trading agent's share of trading fees (Settled.fee_agent).",
};

const breakdownMethodology = {
  Fees: { "AI jobs": "storefront orders (sol_spent + buyback share)", "Trading fees": "mandate Settled fee_agent + fee_protocol" },
  HoldersRevenue: { Buyback: "released buyback shares", "Trading protocol share": "Settled.fee_protocol into the buyback vault" },
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
  breakdownMethodology,
};

export default adapter;
