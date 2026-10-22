// Read handles other route groups publish for the boards (routes/boards.ts), so the boards reuse
// the one trading scan and the one market index of this process instead of a second copy (the
// public Solana devnet RPC is rate-limited per IP). Each is set by its route group when that
// group is configured; unset means "not configured on this server".

import type { MarketIndex } from "../../../apps/explorer/src/market.ts";

export interface SharedTrade {
  chain: "solana" | "cadre"; venue: string; agent: string; account: string; via: "mandate" | "wallet";
  side: "buy" | "sell"; amount_in: string; amount_out: string; unit: string; token: string; at: number; ref: string; house: boolean; flags: string[];
  kind?: string; receipt_post?: string | null; decided_by?: string | null;
}

export const shared: {
  /** Every trade the trading index has read (routes/trading.ts allTrades). */
  trades?: () => Promise<{ solana: SharedTrade[]; cadre: SharedTrade[] }>;
  /** House trader keys (deploy/trading-*.json). */
  tradingHouse?: Record<string, string>;
  /** The services market index (routes/services.ts), refreshed at most every 10 s. */
  market?: () => Promise<MarketIndex>;
  /** House market desk keys (deploy/market-devnet.json). */
  marketHouse?: Record<string, string>;
} = {};
