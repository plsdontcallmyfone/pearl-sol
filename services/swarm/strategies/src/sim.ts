// Simulated execution for practice mode (docs/STRATEGIES.md 3). What is real: the pool reserves
// and fee tier at the quote slot, at the fill slot and at the exit slot (read from mainnet), and
// the constant-product arithmetic with PumpSwap's fee. What is modelled: the signal-to-fill
// latency (MODEL below, from measured devnet fills), and that our order would not have moved
// anyone else's (the pool state after our pretend fill is the real one, not one with our trade
// in it). What is not simulated at all: priority fees, failed sends, MEV, and our impact on later
// observations.
import { buyOut, type Fees, priceE12, sellOut } from "./pump.ts";

/**
 * Signal-to-fill latency: the max of each stage over the 4 own-wallet fills measured on devnet
 * (docs/TRADING.md 5.6): decision -> sent 451 ms, sent -> confirmed 881 ms; 1,332 ms in all,
 * which at Solana's 400 ms slot target is 4 slots (rounded up). A fill is priced at the first
 * mainnet read at least that many slots after the quote slot, and never before the entry
 * commitment is visible on Cadre (preconfirmed or included).
 */
export const MODEL = { decision_to_sent_ms: 451, sent_to_confirmed_ms: 881, slot_ms: 400, latency_slots: 4, source: "docs/TRADING.md 5.6 (max of 4 measured own-wallet fills on devnet)" } as const;

export interface R { base: bigint; quote: bigint }

/** Mid price impact of a buy of `size` lamports, in bps of the mid. */
export function impactBps(r: R, f: Fees, size: bigint): number {
  const out = buyOut(r, f, size);
  if (out === 0n) return 10_000;
  const exec = Number((size * 1_000_000_000_000n) / out);
  const mid = Number(priceE12(r));
  return mid > 0 ? Math.round((exec / mid - 1) * 10_000) : 10_000;
}

export interface Quote { size: string; out: string; price_e12: string; mid_e12: string; impact_bps: number; slot: number }

export function quote(r: R, f: Fees, size: bigint, slot: number): Quote {
  const out = buyOut(r, f, size);
  return { size: size.toString(), out: out.toString(), price_e12: (out > 0n ? (size * 1_000_000_000_000n) / out : 0n).toString(), mid_e12: priceE12(r).toString(), impact_bps: impactBps(r, f, size), slot };
}

export interface Fill { ok: boolean; slot: number; out: string; price_e12: string; slippage_bps: number; reason: string }

/** The buy at the fill slot; refused like a min_out would refuse it when worse than the quote by more than `maxSlipBps`. */
export function fillBuy(q: Quote, r: R, f: Fees, slot: number, maxSlipBps: number): Fill {
  const size = BigInt(q.size);
  const out = buyOut(r, f, size);
  const qo = BigInt(q.out);
  const slip = qo > 0n ? Number(((qo - out) * 10_000n) / qo) : 10_000;
  const price = out > 0n ? (size * 1_000_000_000_000n) / out : 0n;
  if (slip > maxSlipBps || out === 0n) return { ok: false, slot, out: "0", price_e12: price.toString(), slippage_bps: slip, reason: `fill ${slip} bps worse than the quote, above max_slippage_bps ${maxSlipBps}` };
  return { ok: true, slot, out: out.toString(), price_e12: price.toString(), slippage_bps: slip, reason: "filled" };
}

/** Lamports a sale of the whole position returns at reserves `r` (the mark and the exit). */
export const markValue = (r: R, f: Fees, base: bigint) => sellOut(r, f, base);

export type ExitReason = "stop" | "target" | "time";
export const REASON_CODE: Record<ExitReason | "gone" | "no_fill", number> = { stop: 1, target: 2, time: 3, gone: 4, no_fill: 5 };

/** Stop and target on the position's sale value against the size paid (fees and impact included). */
export function exitDue(value: bigint, size: bigint, stopBps: number, targetBps: number, ageMs: number, maxHoldS: number): ExitReason | null {
  if (value * 10_000n <= size * BigInt(10_000 - stopBps)) return "stop";
  if (value * 10_000n >= size * BigInt(10_000 + targetBps)) return "target";
  if (ageMs >= maxHoldS * 1000) return "time";
  return null;
}
