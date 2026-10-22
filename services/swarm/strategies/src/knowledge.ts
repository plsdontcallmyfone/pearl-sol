// Proof of knowledge acquired (docs/STRATEGIES.md 5): for every revised version, the fault rate of
// the bucket it claims to fix, and its practice PnL, on its own trades (all committed after the
// version's commitment height, forward only) against the parent's last trades of the same count.
// A claimed fix counts as LEARNED only when both windows hold at least MIN_FORWARD explained
// trades and the version's rate is lower. Nothing here re-scores a trade with a newer version:
// each trade keeps the buckets its own post-mortem committed. Dependency-free (cadre-api imports it).
import { BUCKET_NAMES, type TradeRec, type VersionRec } from "./records.ts";

/** Explained trades each window needs before a claimed fix can count (design choice, see the doc). */
export const MIN_FORWARD = 10;

export interface Window { trades: number; faults: number; rate: number | null; pnl_lamports: string; wins: number; entry_ids: number[] }
export interface Knowledge {
  version: number;
  parent: number;
  claimed: string;
  motives: number[];
  committed_height: number;
  status: "learned" | "not_learned" | "not_enough_trades" | "baseline_too_small";
  status_text: string;
  min_trades: number;
  window: number;
  forward: Window;
  baseline: Window;
  forward_all: Window;
  fisher_p_one_sided: number | null;
}

const explained = (t: TradeRec) => t.exit !== null && t.postmortem !== null;

function win(ts: TradeRec[], bit: number): Window {
  const faults = ts.filter((t) => (t.postmortem!.buckets & bit) !== 0).length;
  const pnl = ts.reduce((a, t) => a + BigInt(t.exit!.pnl), 0n);
  return { trades: ts.length, faults, rate: ts.length ? faults / ts.length : null, pnl_lamports: pnl.toString(), wins: ts.filter((t) => t.postmortem!.outcome === "win").length, entry_ids: ts.map((t) => t.id) };
}

function logFact(n: number): number {
  let s = 0;
  for (let i = 2; i <= n; i++) s += Math.log(i);
  return s;
}
/** One-sided Fisher exact test: P(forward faults <= observed) with the margins fixed. */
export function fisherLower(fFault: number, fN: number, bFault: number, bN: number): number {
  const K = fFault + bFault, N = fN + bN;
  const hyper = (k: number) => Math.exp(logFact(K) + logFact(N - K) + logFact(fN) + logFact(bN) - logFact(N) - logFact(k) - logFact(K - k) - logFact(fN - k) - logFact(bN - K + k));
  let p = 0;
  for (let k = Math.max(0, K - bN); k <= fFault; k++) p += hyper(k);
  return Math.min(1, p);
}

export function knowledgeOf(v: VersionRec, versions: Map<number, VersionRec>, trades: TradeRec[]): Knowledge | null {
  if (v.parent === 0 || v.claimed === 0) return null;
  const parent = versions.get(v.parent);
  const claimed = BUCKET_NAMES[v.claimed - 1];
  const bit = 1 << (v.claimed - 1);
  // forward only: the version's own explained trades (an entry lands in a later block than its version, enforced by the contract)
  const fwdAll = trades.filter((t) => t.version === v.id && explained(t) && t.height > v.height).sort((a, b) => a.id - b.id);
  const par = trades.filter((t) => t.version === v.parent && explained(t) && t.height < v.height).sort((a, b) => a.id - b.id);
  const n = Math.min(fwdAll.length, par.length);
  const fwd = fwdAll.slice(0, n), base = par.slice(par.length - n);
  const k: Knowledge = {
    version: v.id, parent: v.parent, claimed, motives: v.motives, committed_height: v.height, status: "not_enough_trades", status_text: "", min_trades: MIN_FORWARD, window: n,
    forward: win(fwd, bit), baseline: win(base, bit), forward_all: win(fwdAll, bit), fisher_p_one_sided: null,
  };
  if (!parent) {
    k.status = "baseline_too_small";
    k.status_text = "the parent version is not readable";
  } else if (par.length < MIN_FORWARD) {
    k.status = "baseline_too_small";
    k.status_text = `the parent has ${par.length} explained trades; a claimed fix is scored against at least ${MIN_FORWARD}`;
  } else if (fwdAll.length < MIN_FORWARD) {
    k.status = "not_enough_trades";
    k.status_text = `not enough trades yet: ${fwdAll.length} of ${MIN_FORWARD} explained trades since height ${v.height}`;
  } else {
    k.fisher_p_one_sided = fisherLower(k.forward.faults, k.forward.trades, k.baseline.faults, k.baseline.trades);
    const lower = (k.forward.rate ?? 1) < (k.baseline.rate ?? 0);
    k.status = lower ? "learned" : "not_learned";
    k.status_text = lower
      ? `learned: ${claimed} faults fell from ${k.baseline.faults}/${n} to ${k.forward.faults}/${n} trades (one-sided Fisher p = ${k.fisher_p_one_sided.toFixed(3)})`
      : `not learned: ${claimed} faults ${k.forward.faults}/${n} after against ${k.baseline.faults}/${n} before`;
  }
  return k;
}

export interface VersionRecord {
  id: number;
  line: number;
  parent: number;
  author: string;
  hash: string;
  height: number;
  head: boolean;
  record: { trades: number; wins: number; losses: number; flats: number; no_fills: number; win_rate: number | null; pnl_lamports: string; lucky_wins: number; buckets: Record<string, number> };
  knowledge: Knowledge | null;
}

/** The verified record of every version, straight from the contract's own counters. */
export function records(versions: VersionRec[], trades: TradeRec[]): VersionRecord[] {
  const byId = new Map(versions.map((v) => [v.id, v]));
  return versions.map((v) => {
    const s = v.stats;
    const decided = s.wins + s.losses + s.flats;
    return {
      id: v.id, line: v.line, parent: v.parent, author: v.author, hash: v.hash, height: v.height, head: v.superseded_by === 0,
      record: { trades: s.trades, wins: s.wins, losses: s.losses, flats: s.flats, no_fills: s.no_fills, win_rate: decided ? s.wins / decided : null, pnl_lamports: s.pnl_sum, lucky_wins: s.lucky_wins, buckets: s.buckets },
      knowledge: knowledgeOf(v, byId, trades),
    };
  });
}
