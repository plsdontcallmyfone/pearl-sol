// A strategy version (docs/STRATEGIES.md 1): a readable, versioned rule set. The canonical JSON
// (sorted keys, no spaces) is the blob whose blake3 is committed on Cadre as the version hash, so
// anyone holding the blob can check it is the version that traded.
import { blake3 } from "../../src/blake3.ts";

export const SPEC_FORMAT = "cadre-strategy-v1";

/** Signals a condition may read. Every one is computed from mainnet reads (src/signals.ts). */
export const SIGNALS = {
  pool_sol: "SOL on the pool's quote side (tradeable reserve, virtual quote included)",
  mcap_sol: "market cap in SOL as pump prices fee tiers (quote x supply / base)",
  fee_bps: "PumpSwap fee tier for the pool now (LP + protocol + creator, bps)",
  ret_5m: "price change over the last 5 minutes (fraction)",
  ret_15m: "price change over the last 15 minutes (fraction)",
  ret_60m: "price change over the last 60 minutes (fraction)",
  vol_15m: "standard deviation of log returns between observations over 15 minutes",
  flow_15m: "change of the pool's SOL side over 15 minutes as a fraction of it (net buying > 0)",
  top10_share: "share of supply in the 10 largest holder accounts, pool vaults excluded",
  top1_share: "share of supply in the largest holder account, pool vaults excluded",
  dev_share: "share of supply the coin creator's token account holds",
  dev_sold_15m: "1 when the coin creator's balance fell during the last 15 minutes, else 0",
  lp_wallet_share: "share of LP tokens held by plain wallets (they can pull liquidity)",
} as const;
export type Signal = keyof typeof SIGNALS;
export const SIGNAL_NAMES = Object.keys(SIGNALS) as Signal[];

export interface Condition { signal: Signal; op: ">=" | "<="; value: number }

export interface StrategySpec {
  format: typeof SPEC_FORMAT;
  name: string;
  author: string;
  /** Version hash of the parent, or null for the first version of a line. */
  parent: string | null;
  venue: "pumpswap";
  mode: "practice";
  /** Pretend starting balance of the line (lamports): practice equity = this + committed PnL. */
  practice_balance_lamports: number;
  /** All must hold at the decision slot. */
  conditions: Condition[];
  size: { equity_fraction_bps: number; max_lamports: number; max_pool_share_bps: number };
  stop_bps: number;
  target_bps: number;
  max_hold_s: number;
  /** Most of the practice equity one trade may lose (bps). Size is cut so size x stop <= this. */
  max_risk_bps: number;
  /** Fill refused (no fill) when the fill is worse than the decision quote by more than this. */
  max_slippage_bps: number;
  /** The stated edge: what the author expects, checked by the THESIS and VARIANCE buckets. */
  edge: { win_rate: number; avg_win_bps: number; avg_loss_bps: number };
  /** Why, in the author's words (from the paid authoring thought). */
  rationale: string;
  /** What changed from the parent and why (empty for a first version). */
  change: string;
  /** Receipt id of the paid authoring thought. */
  authored_by: string;
}

function canon(x: unknown): unknown {
  if (Array.isArray(x)) return x.map(canon);
  if (x && typeof x === "object") {
    const o: Record<string, unknown> = {};
    for (const k of Object.keys(x as object).sort()) o[k] = canon((x as any)[k]);
    return o;
  }
  return x;
}
export const canonicalJson = (x: unknown) => JSON.stringify(canon(x));
export const blobId = (bytes: Uint8Array | string) => Buffer.from(blake3(typeof bytes === "string" ? Buffer.from(bytes, "utf8") : bytes)).toString("hex");
export const specBytes = (s: StrategySpec) => Buffer.from(canonicalJson(s), "utf8");
export const specHash = (s: StrategySpec) => blobId(specBytes(s));
/** The condition set's identity: the THESIS base rate counts past firings of exactly this set. */
export const conditionsHash = (c: Condition[]) => blobId(canonicalJson([...c].sort((a, b) => (a.signal + a.op).localeCompare(b.signal + b.op))));

/** Signals the strategy inspects: its conditions, plus what the size rule and the quote read. */
export function inspected(s: StrategySpec): Set<Signal> {
  const set = new Set<Signal>(s.conditions.map((c) => c.signal));
  set.add("pool_sol");
  set.add("fee_bps");
  return set;
}

/** Bounds every spec must satisfy (proposed defaults; the runner refuses anything outside). */
export function validate(s: StrategySpec): string[] {
  const e: string[] = [];
  if (s.format !== SPEC_FORMAT) e.push("format");
  if (s.venue !== "pumpswap" || s.mode !== "practice") e.push("venue pumpswap, mode practice");
  if (!(s.conditions.length >= 1 && s.conditions.length <= 12)) e.push("1..12 conditions");
  for (const c of s.conditions) if (!(c.signal in SIGNALS) || !(c.op === ">=" || c.op === "<=") || !Number.isFinite(c.value)) e.push(`condition ${JSON.stringify(c)}`);
  if (!(s.stop_bps >= 100 && s.stop_bps <= 5_000)) e.push("stop_bps 100..5000");
  if (!(s.target_bps >= 100 && s.target_bps <= 20_000)) e.push("target_bps 100..20000");
  if (!(s.max_hold_s >= 120 && s.max_hold_s <= 86_400)) e.push("max_hold_s 120..86400");
  if (!(s.max_risk_bps >= 10 && s.max_risk_bps <= 500)) e.push("max_risk_bps 10..500");
  if (!(s.max_slippage_bps >= 10 && s.max_slippage_bps <= 1_000)) e.push("max_slippage_bps 10..1000");
  if (!(s.size.equity_fraction_bps >= 10 && s.size.equity_fraction_bps <= 5_000)) e.push("equity_fraction_bps 10..5000");
  if (!(s.size.max_pool_share_bps >= 1 && s.size.max_pool_share_bps <= 200)) e.push("max_pool_share_bps 1..200");
  if (!(s.size.max_lamports > 0)) e.push("max_lamports");
  if (!(s.edge.win_rate > 0 && s.edge.win_rate < 1)) e.push("edge.win_rate in (0, 1)");
  if (!(s.practice_balance_lamports > 0)) e.push("practice_balance_lamports");
  return e;
}

export type Values = Partial<Record<Signal, number | null>>;

/** Every condition holds (a missing signal never passes). */
export function holds(s: StrategySpec, v: Values): { ok: boolean; failed: Condition[] } {
  const failed = s.conditions.filter((c) => {
    const x = v[c.signal];
    if (x === null || x === undefined || !Number.isFinite(x)) return true;
    return c.op === ">=" ? !(x >= c.value) : !(x <= c.value);
  });
  return { ok: failed.length === 0, failed };
}

/** The size rule: min(equity x fraction, max_risk / stop, pool x share, max_lamports). */
export function sizeFor(s: StrategySpec, equity: bigint, poolLamports: bigint): { size: bigint; maxRisk: bigint; bound: string } {
  const maxRisk = (equity * BigInt(s.max_risk_bps)) / 10_000n;
  const c: [bigint, string][] = [
    [(equity * BigInt(s.size.equity_fraction_bps)) / 10_000n, "equity fraction"],
    [(maxRisk * 10_000n) / BigInt(s.stop_bps), "max risk / stop"],
    [(poolLamports * BigInt(s.size.max_pool_share_bps)) / 10_000n, "pool share"],
    [BigInt(s.size.max_lamports), "max size"],
  ];
  c.sort((a, b) => (a[0] < b[0] ? -1 : a[0] > b[0] ? 1 : 0));
  return { size: c[0][0] > 0n ? c[0][0] : 0n, maxRisk, bound: c[0][1] };
}
