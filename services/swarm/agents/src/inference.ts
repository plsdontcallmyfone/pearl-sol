// Prepaid inference for hosted user agents: how big a top-up ticket to buy, when a refused top-up
// or a used-up row budget waits, and how often practice trading may ask for a paid decision.
//
// Measured on box 0 (2026-10-23, gateway POST /v1/balance/topup quotes): a balance ticket's x402
// price is almost all a fixed per-order cost: 25,135,765 lamports for 1,024 rows, 25,209,724 for
// 4,096 and 25,308,335 for 8,192 (the gateway's largest: "rows must be a multiple of 4 in [4,
// 8192]", --max-balance-rows 8192 and the chain's max_balance_rows 8192). Buying just enough rows
// for the one thought that got a 402 (the old N + 256 rule) paid that fixed cost every thought or
// two, and agents hit their daily SOL cap after 1 to 3 top-ups. So a top-up buys the largest
// ticket the daily SOL cap and the wallet allow, measured against the live quote of each size.

/** The gateway's largest balance ticket when the chain says nothing (box 0: 8192). */
export const MAX_TICKET_ROWS = 8192;
/** Lamports a top-up leaves in the wallet beyond the price: rent-exempt minimum of a system
 * account (890,880) plus a few Solana fees, so a payment never fails on rent. */
export const TOPUP_SOL_RESERVE = 1_000_000n;
/** Rows a practice thought takes when the agent has no receipts yet today: the upper end of the
 * 205 to 400 rows the box 0 receipts show per practice decision (2026-10-23 journal). */
export const PRACTICE_ROWS_GUESS = 400;
/** How often a wallet-refused top-up re-reads the wallet balance (one cadre-api GET). */
export const WALLET_RECHECK_MS = 60_000;

const DAY_MS = 86_400_000;
const up4 = (n: number) => Math.ceil(n / 4) * 4;

/** Start of the next UTC day after `now` (ms). */
export const nextUtcDay = (now: number) => Math.floor(now / DAY_MS) * DAY_MS + DAY_MS;

/** Ticket sizes to try, largest first: the gateway's largest, the runtime's default, and the least
 * that serves a `need`-row thought with `minFree` rows to spare. Every size is a multiple of 4,
 * at most `maxRows` and at least that least; empty when even the largest ticket is too small. */
export function ticketLadder(need: number, o: { defaultRows: number; maxRows: number; minFree: number }): number[] {
  const max = Math.floor(o.maxRows / 4) * 4;
  const least = up4(Math.max(4, need + o.minFree));
  if (least > max) return [];
  const out = [max, Math.min(max, up4(o.defaultRows)), least].filter((x) => x >= least);
  return [...new Set(out)].sort((a, b) => b - a);
}

/** Why thinking waits: the daily SOL cap (until the next UTC day), the wallet (until its balance
 * changes), or a thought bigger than the largest ticket (until the next UTC day). */
export interface Hold {
  reason: "sol_cap" | "wallet" | "too_big";
  since: number;
  /** ms; 0 = until the wallet balance changes */
  until: number;
  lamports?: string;
  checkedAt?: number;
  need: number;
  price?: string;
}

/** The inference state a runner keeps across restarts (the agent's state file). */
export interface InferenceState {
  hold: Hold | null;
  /** last quote seen: rows and price (lamports), for the practice cadence */
  quote?: { rows: number; price: string; at: number };
  /** when the last practice thought ran (ms) */
  practiceAt?: number;
  /** UTC day of the last "row budget used" feed line */
  rowsNoted?: string;
}

/** The feed line for a hold (one per hold). */
export function holdText(h: Hold, o: { today: string; cap: string; wallet?: string }): string {
  if (h.reason === "sol_cap") return `paused its paid thinking until 00:00 UTC: a ${h.price ?? "?"}-lamport inference top-up would pass its daily SOL cap (${o.today} of ${o.cap} lamports spent today)`;
  if (h.reason === "wallet") return `paused its paid thinking until its wallet holds more SOL: a ${h.price ?? "?"}-lamport inference top-up needs more than the ${o.wallet ?? "?"} lamports it holds`;
  return `paused its paid thinking until 00:00 UTC: a ${h.need}-row thought is larger than the largest prepaid ticket`;
}

/** Practice cadence: ms until the next practice thought may run (0 = now). The day's rows left
 * (the spec's rows_per_day, and the rows the rest of the daily SOL cap still buys at the last
 * quote) are spread evenly over what is left of the UTC day at `avgRows` rows a thought. */
export function practiceWaitMs(o: {
  now: number; lastAt: number; avgRows: number; rowsLeft: number;
  rowsFree: number; capLeft: bigint; quote?: { rows: number; price: string };
}): number {
  const toDay = nextUtcDay(o.now) - o.now;
  let left = o.rowsLeft;
  if (o.quote && BigInt(o.quote.price) > 0n) {
    const tickets = o.capLeft > 0n ? Number(o.capLeft / BigInt(o.quote.price)) : 0;
    left = Math.min(left, Math.max(0, o.rowsFree) + tickets * o.quote.rows);
  }
  const avg = Math.max(1, o.avgRows);
  if (left < avg) return toDay;
  if (!o.lastAt) return 0;
  const every = Math.floor((toDay * avg) / left);
  return Math.max(0, o.lastAt + every - o.now);
}

/** "no prepaid balance with N free rows" in a gateway 402: N, else null. */
export function need402(e: unknown): number | null {
  const m = /\b402\b[\s\S]*with (\d+) free rows/.exec(String((e as Error)?.message ?? e));
  return m ? Number(m[1]) : null;
}
