// Board math (docs/ECOSYSTEM.md 2.8, Phase 5): pure functions over "facts", one fact per paid
// unit of evidence (a completed hire, an answered paid DM, a tip, a realized sell, a mined block,
// a verdict). Collectors in boards/collect.ts build the facts from chain data only; this module
// windows them, screens them and ranks them. No I/O, so the tests drive it with synthetic facts.
//
// Rules every board shares:
//   - a fact with `excluded` set (self-dealing: the same rules as the explorer indexer and the
//     market, see collect.ts) never counts; its amount is reported as `excluded_value`;
//   - a fact with `house` set (paid by a declared house account) is house activity: it is
//     reported as `house_value` and never inside `value` (the organic figure);
//   - a row is ranked only when it reaches the board's minimum sample (`min_sample` of
//     `sample_unit`, and `min_payers` distinct payers); rows below it are counted, not ranked;
//   - windows are wall-clock: 24 h and 7 d end now, `all` is everything indexed.

export type Window = "24h" | "7d" | "all";
export const WINDOWS: Record<Window, number | null> = { "24h": 86_400_000, "7d": 7 * 86_400_000, all: null };
export type Side = "organic" | "house" | "all";

export interface Ref {
  /** "cadre" (a chain tx hash), "solana" (a signature), "market" (a hire id) */
  chain: "cadre" | "solana" | "market";
  id: string;
  what: string;
}

export interface Fact {
  account: string;
  /** money or units credited to `account` (may be negative for realized PnL) */
  amount: bigint;
  /** the paying counterparty, for distinct payers (null where there is none, e.g. a sell) */
  payer: string | null;
  /** ms since epoch (block time), null when unknown (counted only in `all`) */
  at: number | null;
  ref: Ref;
  house: boolean;
  excluded: string | null;
  heuristic?: boolean;
  /** rate boards: the outcome this fact records */
  outcome?: "accepted" | "rejected" | "expired";
  /** side figures summed per row (rows, lamports, blocks ...) */
  extra?: Record<string, bigint>;
  /** counted only in `extra` (e.g. an unanswered paid DM, an unpaid follower): no value, no sample */
  tally?: boolean;
}

export function inWindow(at: number | null, w: Window, now: number): boolean {
  const span = WINDOWS[w];
  if (span === null) return true;
  return at !== null && at > now - span && at <= now + 120_000;
}

export interface Agg {
  account: string;
  value: bigint;
  house_value: bigint;
  excluded_value: bigint;
  /** organic facts (screened, not house) */
  n: number;
  /** house facts */
  n_house: number;
  n_excluded: number;
  payers: Map<string, bigint>;
  house_payers: Set<string>;
  accepted: number;
  outcomes: number;
  house_accepted: number;
  house_outcomes: number;
  extra: Record<string, bigint>;
  refs: (Ref & { at: number | null; amount: string })[];
  heuristic_excluded: number;
  last_at: number | null;
}

const newAgg = (account: string): Agg => ({
  account, value: 0n, house_value: 0n, excluded_value: 0n, n: 0, n_house: 0, n_excluded: 0, payers: new Map(), house_payers: new Set(),
  accepted: 0, outcomes: 0, house_accepted: 0, house_outcomes: 0, extra: {}, refs: [], heuristic_excluded: 0, last_at: null,
});

/** Sums facts per account inside the window. */
export function aggregate(facts: Fact[], w: Window, now: number): Map<string, Agg> {
  const out = new Map<string, Agg>();
  for (const f of facts) {
    if (!inWindow(f.at, w, now)) continue;
    const a = out.get(f.account) ?? newAgg(f.account);
    out.set(f.account, a);
    if (f.tally) {
      if (!f.excluded) for (const [k, v] of Object.entries(f.extra ?? {})) a.extra[k] = (a.extra[k] ?? 0n) + v;
      continue;
    }
    if (f.excluded) {
      a.excluded_value += f.amount;
      a.n_excluded++;
      if (f.heuristic) a.heuristic_excluded++;
      continue;
    }
    if (f.at !== null && (a.last_at === null || f.at > a.last_at)) a.last_at = f.at;
    for (const [k, v] of Object.entries(f.extra ?? {})) a.extra[k] = (a.extra[k] ?? 0n) + v;
    if (f.house) {
      a.house_value += f.amount;
      a.n_house++;
      if (f.payer) a.house_payers.add(f.payer);
      if (f.outcome) {
        a.house_outcomes++;
        if (f.outcome === "accepted") a.house_accepted++;
      }
      continue;
    }
    a.value += f.amount;
    a.n++;
    if (f.payer) a.payers.set(f.payer, (a.payers.get(f.payer) ?? 0n) + (f.amount > 0n ? f.amount : 0n));
    if (f.outcome) {
      a.outcomes++;
      if (f.outcome === "accepted") a.accepted++;
    }
    a.refs.push({ ...f.ref, at: f.at, amount: f.amount.toString() });
  }
  for (const a of out.values()) {
    a.refs.sort((x, y) => (y.at ?? 0) - (x.at ?? 0));
    a.refs = a.refs.slice(0, 5);
  }
  return out;
}

export interface BoardSpec {
  /** what is ranked: "sum" of amounts, "rate" (accepted / outcomes, ppm), or "count" of facts */
  kind: "sum" | "rate" | "count";
  /** facts (or outcomes for a rate) a row needs before it is ranked */
  min_sample: number;
  /** distinct payers a row needs before it is ranked (0 = no payer rule) */
  min_payers: number;
  /** rank only rows with a figure above zero (every board but realized PnL, where losers stay listed) */
  positive?: boolean;
}

export interface Ranked {
  rank: number;
  agg: Agg;
  /** the ranked figure: organic sum, organic rate in ppm, or organic count */
  score: bigint;
  /** house side figure for the same row (house_value, house rate, house count) */
  house_score: bigint;
  sample: number;
}

/** Score of one row on one side: organic (screened, not house) or house. */
export function score(a: Agg, spec: BoardSpec, side: "organic" | "house"): { score: bigint; sample: number; payers: number } {
  const org = side === "organic";
  if (spec.kind === "rate") {
    const n = org ? a.outcomes : a.house_outcomes;
    const ok = org ? a.accepted : a.house_accepted;
    return { score: n === 0 ? 0n : (BigInt(ok) * 1_000_000n) / BigInt(n), sample: n, payers: org ? a.payers.size : a.house_payers.size };
  }
  if (spec.kind === "count") return { score: BigInt(org ? a.n : a.n_house), sample: org ? a.n : a.n_house, payers: org ? a.payers.size : a.house_payers.size };
  return { score: org ? a.value : a.house_value, sample: org ? a.n : a.n_house, payers: org ? a.payers.size : a.house_payers.size };
}

export function eligible(s: { sample: number; payers: number }, spec: BoardSpec): boolean {
  return s.sample >= spec.min_sample && s.payers >= spec.min_payers;
}

/**
 * Ranks rows. `side` picks the rows by their label (organic: accounts not labelled house; house:
 * house accounts only; all: both, labelled) and the money class ranked: organic and all rank on
 * the organic figure only; the house side ranks house accounts on their house figure.
 */
export function rank(aggs: Map<string, Agg>, spec: BoardSpec, side: Side, isHouse: (a: string) => boolean): { rows: Ranked[]; below_min: number } {
  const rows: Ranked[] = [];
  let below = 0;
  for (const a of aggs.values()) {
    const house = isHouse(a.account);
    if (side === "organic" && house) continue;
    if (side === "house" && !house) continue;
    const cls = side === "house" ? "house" : "organic";
    const s = score(a, spec, cls);
    const other = score(a, spec, cls === "house" ? "organic" : "house");
    if (spec.positive && s.score <= 0n) continue;
    if (!eligible(s, spec)) {
      if (s.sample > 0 || other.sample > 0) below++;
      continue;
    }
    rows.push({ rank: 0, agg: a, score: s.score, house_score: other.score, sample: s.sample });
  }
  rows.sort((x, y) => (y.score > x.score ? 1 : y.score < x.score ? -1 : 0) || y.sample - x.sample || (x.agg.account < y.agg.account ? -1 : 1));
  rows.forEach((r, i) => (r.rank = i + 1));
  return { rows, below_min: below };
}

/** Largest single payer's share of the organic value, in bps (null without payers). */
export function topPayerShareBps(a: Agg): number | null {
  if (a.payers.size === 0) return null;
  let tot = 0n, top = 0n;
  for (const v of a.payers.values()) {
    tot += v;
    if (v > top) top = v;
  }
  return tot === 0n ? null : Number((top * 10_000n) / tot);
}

// ------------------------------------------------------------------ trading

export interface Fill {
  agent: string;
  account: string;
  token: string;
  side: "buy" | "sell";
  amount_in: bigint;
  amount_out: bigint;
  at: number;
  ref: string;
  house: boolean;
  flags: string[];
}

/**
 * Realized PnL of every sell, by average cost over the fills of one (account, token) book in
 * time order: the same arithmetic as routes/trading.ts avgCostPnl, but per sell, so a window can
 * sum only the sells inside it. A sell beyond the open position realizes only its covered part.
 */
export function realizedPerSell(fills: Fill[]): { fill: Fill; realized: bigint }[] {
  const books = new Map<string, { pos: bigint; cost: bigint }>();
  const out: { fill: Fill; realized: bigint }[] = [];
  for (const f of [...fills].sort((a, b) => a.at - b.at || (a.ref < b.ref ? -1 : 1))) {
    const k = `${f.account}|${f.token}`;
    const b = books.get(k) ?? { pos: 0n, cost: 0n };
    books.set(k, b);
    if (f.side === "buy") {
      b.pos += f.amount_out;
      b.cost += f.amount_in;
      continue;
    }
    if (b.pos <= 0n || f.amount_in === 0n) {
      out.push({ fill: f, realized: 0n });
      continue;
    }
    const take = f.amount_in > b.pos ? b.pos : f.amount_in;
    const basis = (b.cost * take) / b.pos;
    const realized = (f.amount_out * take) / f.amount_in - basis;
    b.cost -= basis;
    b.pos -= take;
    out.push({ fill: f, realized });
  }
  return out;
}

export function median(xs: number[]): number | null {
  if (!xs.length) return null;
  const v = [...xs].sort((a, b) => a - b);
  return v[Math.floor((v.length - 1) / 2)];
}
