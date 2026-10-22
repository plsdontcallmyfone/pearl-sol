// The four paid Cadre thoughts of a practice agent (docs/STRATEGIES.md): author a first version,
// decide an entry, write the lesson of a post-mortem, choose a fix. Each is one gateway request
// whose id is committed on Cadre, so anyone can fetch the receipt and replay it (cadre-glue-v1,
// docs/PROOFS.md 11). The model on devnet is a 1B int7 Llama: it chooses among bounded options
// and writes short text; the numbers and every rule are this code's. Prompts hold only numbers
// computed here from mainnet reads and the agent's own committed record, never text written by
// other accounts (the trading prompt-injection rule, docs/TRADING.md 7).
import { checkOutput } from "../../src/filter.ts";
import { BUCKETS, type Bucket, RED_FLAGS, type Result } from "./analyzer.ts";
import type { Condition, Signal, StrategySpec, Values } from "./spec.ts";

export type Msg = { role: string; content: string };
export interface ThoughtOut { text: string; requestId: string }
export type Think = (messages: Msg[], maxTokens: number) => Promise<ThoughtOut>;

const n4 = (x: number | null | undefined) => (x === null || x === undefined ? "unknown" : Number(x.toPrecision(4)).toString());

// ------------------------------------------------------------ authoring a first version

export const STOPS = [500, 800, 1200];
export const TARGETS = [800, 1500, 2500];
export const HOLDS_MIN = [10, 20, 40];

export function authorPrompt(persona: string, base: { conditions: Condition[] }): Msg[] {
  const conds = base.conditions.map((c) => `${c.signal} ${c.op} ${c.value}`).join("; ");
  return [
    { role: "system", content: `You are ${persona}, a trading agent writing your own spot strategy for memecoins on PumpSwap. Practice money, real prices. Answer on one line exactly like: STOP 8 TARGET 15 HOLD 20 WIN 45 because <short reason>` },
    { role: "user", content: `Your entry rules: ${conds}. Choose a stop loss of 5, 8 or 12 percent, a profit target of 8, 15 or 25 percent, a maximum hold of 10, 20 or 40 minutes, and the share of trades you expect to win (20 to 70 percent).` },
  ];
}

const pick = (x: number | undefined, menu: number[], def: number) => (x === undefined || !Number.isFinite(x) ? def : menu.reduce((a, b) => (Math.abs(b - x) < Math.abs(a - x) ? b : a)));

export function parseAuthor(text: string): { stop_bps: number; target_bps: number; max_hold_s: number; win_rate: number; rationale: string } {
  const t = text.toUpperCase();
  const num = (k: string) => { const m = t.match(new RegExp(`${k}\\D{0,6}(\\d{1,3})`)); return m ? Number(m[1]) : undefined; };
  const stop = pick(num("STOP") !== undefined ? num("STOP")! * 100 : undefined, STOPS, 800);
  const target = pick(num("TARGET") !== undefined ? num("TARGET")! * 100 : undefined, TARGETS, 1500);
  const hold = pick(num("HOLD"), HOLDS_MIN, 20);
  const w = num("WIN");
  const win = w === undefined ? 40 : Math.min(70, Math.max(20, w));
  const m = text.match(/because\s+(.{1,200})/i);
  const reason = m ? m[1].trim() : "";
  return { stop_bps: stop, target_bps: target, max_hold_s: hold * 60, win_rate: win / 100, rationale: checkOutput(reason, { maxBytes: 240 }).ok ? reason : "" };
}

// ------------------------------------------------------------ deciding an entry

export function decisionPrompt(persona: string, spec: StrategySpec, v: Values, size: bigint): Msg[] {
  const lines = Object.entries(v).map(([k, x]) => `${k} ${n4(x as number | null)}`).join(", ");
  return [
    { role: "system", content: `You are ${persona}, trading practice money at real prices. Your rules all hold for this pool. Answer with one word, TAKE or SKIP, then a short reason.` },
    { role: "user", content: `Pool numbers: ${lines}. Size ${Number(size) / 1e9} SOL, stop ${spec.stop_bps / 100}%, target ${spec.target_bps / 100}%. TAKE or SKIP?` },
  ];
}

/** The rules decided; the thought can only veto. Anything but a clear SKIP first is TAKE. */
export function parseDecision(text: string): "take" | "skip" {
  const t = text.toUpperCase();
  const s = t.indexOf("SKIP"), k = t.indexOf("TAKE");
  return s >= 0 && (k < 0 || s < k) ? "skip" : "take";
}

// ------------------------------------------------------------ the lesson of a post-mortem

export function lessonPrompt(persona: string, r: Result): Msg[] {
  const lines = r.verdicts.filter((x) => x.status !== "not_applicable").map((x) => `${x.bucket}: ${x.status}. ${x.evidence[0] ?? ""}`).join("\n");
  return [
    { role: "system", content: `You are ${persona}. You review your own practice trade honestly. Write one sentence starting with LESSON:` },
    { role: "user", content: `Outcome ${r.outcome}, PnL ${Number(r.pnl_lamports) / 1e9} SOL. Buckets at fault: ${r.names.join(", ") || "none"}.\n${lines}` },
  ];
}

export function parseLesson(text: string): string {
  const m = text.match(/LESSON:?\s*(.{1,280})/i);
  const s = (m ? m[1] : text).split("\n")[0].trim().slice(0, 280);
  return checkOutput(s, { maxBytes: 300 }).ok ? s : "";
}

// ------------------------------------------------------------ choosing a fix

export interface Fix { key: string; text: string; apply: (s: StrategySpec) => StrategySpec }

const setCond = (s: StrategySpec, c: Condition): StrategySpec => ({ ...s, conditions: [...s.conditions.filter((x) => !(x.signal === c.signal && x.op === c.op)), c] });
const tighten = (c: Condition, f: number): Condition => ({ ...c, value: Number((c.op === ">=" ? (c.value >= 0 ? c.value * (1 + f) : c.value * (1 - f)) : c.value >= 0 ? c.value * (1 - f) : c.value * (1 + f)).toPrecision(4)) });
const flagCond = (sig: Signal): Condition | null => {
  const f = RED_FLAGS.find((x) => x.signal === sig);
  return f ? { signal: sig, op: f.op === ">" ? "<=" : ">=", value: f.value } : null;
};

/**
 * Candidate fixes for a bucket, built from the evidence of the cited post-mortems. Deterministic;
 * the paid thought only picks the letter.
 */
export function fixesFor(b: Bucket, s: StrategySpec, ev: { missed: Signal[]; observedWinRate: number | null; sigmaHoldBps: number | null; medianVol: number | null }): Fix[] {
  const out: Fix[] = [];
  if (b === "INFORMATION") {
    for (const sig of ev.missed.slice(0, 3)) {
      const c = flagCond(sig);
      if (c) out.push({ key: `inspect ${sig}`, text: `inspect ${sig}: add the condition ${sig} ${c.op} ${c.value}`, apply: (x) => setCond(x, c) });
    }
  } else if (b === "THESIS") {
    if (ev.observedWinRate !== null) {
      const w = Math.max(0.05, Math.round(ev.observedWinRate * 20) / 20);
      out.push({ key: "restate edge", text: `state the edge the record shows: win rate ${Math.round(w * 100)}%`, apply: (x) => ({ ...x, edge: { ...x.edge, win_rate: w } }) });
    }
    const first = s.conditions.find((c) => c.signal.startsWith("ret_") || c.signal === "flow_15m");
    if (first) out.push({ key: `tighten ${first.signal}`, text: `tighten ${first.signal} by 25%`, apply: (x) => setCond(x, tighten(first, 0.25)) });
    const pool = s.conditions.find((c) => c.signal === "pool_sol" && c.op === ">=");
    out.push({ key: "deeper pools", text: `trade deeper pools only: pool_sol >= ${pool ? Math.round(pool.value * 1.5) : 50}`, apply: (x) => setCond(x, { signal: "pool_sol", op: ">=", value: pool ? Math.round(pool.value * 1.5) : 50 }) });
  } else if (b === "EXECUTION") {
    out.push({ key: "smaller pool share", text: `halve the pool share per trade to ${Math.max(1, Math.floor(s.size.max_pool_share_bps / 2))} bps`, apply: (x) => ({ ...x, size: { ...x.size, max_pool_share_bps: Math.max(1, Math.floor(x.size.max_pool_share_bps / 2)) } }) });
    const pool = s.conditions.find((c) => c.signal === "pool_sol" && c.op === ">=");
    out.push({ key: "deeper pools", text: `trade deeper pools only: pool_sol >= ${pool ? Math.round(pool.value * 1.5) : 50}`, apply: (x) => setCond(x, { signal: "pool_sol", op: ">=", value: pool ? Math.round(pool.value * 1.5) : 50 }) });
    out.push({ key: "wider slippage", text: `accept fills up to ${Math.min(1000, Math.round(s.max_slippage_bps * 1.5))} bps from the quote`, apply: (x) => ({ ...x, max_slippage_bps: Math.min(1000, Math.round(x.max_slippage_bps * 1.5)) }) });
  } else if (b === "SIZING") {
    out.push({ key: "smaller size", text: `halve the equity fraction per trade to ${Math.max(10, Math.floor(s.size.equity_fraction_bps / 2))} bps`, apply: (x) => ({ ...x, size: { ...x.size, equity_fraction_bps: Math.max(10, Math.floor(x.size.equity_fraction_bps / 2)) } }) });
    out.push({ key: "smaller pool share", text: `halve the pool share per trade to ${Math.max(1, Math.floor(s.size.max_pool_share_bps / 2))} bps`, apply: (x) => ({ ...x, size: { ...x.size, max_pool_share_bps: Math.max(1, Math.floor(x.size.max_pool_share_bps / 2)) } }) });
    out.push({ key: "lower max risk", text: `lower the max risk per trade to ${Math.max(10, Math.floor(s.max_risk_bps / 2))} bps of equity`, apply: (x) => ({ ...x, max_risk_bps: Math.max(10, Math.floor(x.max_risk_bps / 2)) }) });
  } else if (b === "RISK_MANAGEMENT") {
    if (ev.sigmaHoldBps !== null) {
      const stop = Math.min(5000, Math.max(s.stop_bps + 100, Math.ceil((ev.sigmaHoldBps * 1.5) / 100) * 100));
      out.push({ key: "stop outside the noise", text: `widen the stop to ${stop / 100}% (1.5 standard deviations of the hold)`, apply: (x) => ({ ...x, stop_bps: stop }) });
    }
    out.push({ key: "wider stop and target", text: `widen stop and target by half (${Math.round(s.stop_bps * 1.5) / 100}% / ${Math.round(s.target_bps * 1.5) / 100}%)`, apply: (x) => ({ ...x, stop_bps: Math.min(5000, Math.round(x.stop_bps * 1.5)), target_bps: Math.min(20000, Math.round(x.target_bps * 1.5)) }) });
    if (ev.medianVol !== null) {
      const v = Number(ev.medianVol.toPrecision(3));
      out.push({ key: "calmer pools", text: `only enter when vol_15m <= ${v}`, apply: (x) => setCond(x, { signal: "vol_15m", op: "<=", value: v }) });
    }
  }
  return out.slice(0, 3);
}

export function fixPrompt(persona: string, b: Bucket, fixes: Fix[], cited: number): Msg[] {
  const letters = fixes.map((f, i) => `${"ABC"[i]}: ${f.text}`).join("\n");
  return [
    { role: "system", content: `You are ${persona}. Your post-mortems found a fixable fault and you revise your own strategy. Answer with one letter, then a short reason.` },
    { role: "user", content: `${cited} post-mortem(s) named ${b}. Options:\n${letters}\nWhich one?` },
  ];
}

export function parseFix(text: string, n: number): number {
  const m = text.toUpperCase().match(/\b([ABC])\b/);
  const i = m ? "ABC".indexOf(m[1]) : 0;
  return i >= 0 && i < n ? i : 0;
}

export const BUCKET_LIST = BUCKETS;
