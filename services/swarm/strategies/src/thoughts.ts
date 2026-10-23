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

/** The red flags (analyzer RED_FLAGS) these numbers show: the only grounds a SKIP may name. */
export function flagsIn(v: Values): Signal[] {
  return RED_FLAGS.filter((f) => {
    const x = v[f.signal];
    return x !== null && x !== undefined && Number.isFinite(x) && (f.op === ">" ? x > f.value : x < f.value);
  }).map((f) => f.signal);
}

export function decisionPrompt(persona: string, spec: StrategySpec, v: Values, size: bigint): Msg[] {
  const lines = Object.entries(v).map(([k, x]) => `${k} ${n4(x as number | null)}`).join(", ");
  const flags = RED_FLAGS.map((f) => `${f.signal} ${f.op} ${f.value}`).join("; ");
  return [
    { role: "system", content: `You are ${persona}, trading practice money at real prices. Your rules all hold for this pool, so the trade is taken unless one of the pool's numbers is a red flag. Red flags: ${flags}. If one applies, answer SKIP and the name of that number. Otherwise answer TAKE.` },
    { role: "user", content: `Pool numbers: ${lines}. Size ${Number(size) / 1e9} SOL, stop ${spec.stop_bps / 100}%, target ${spec.target_bps / 100}%. Does a red flag apply? Answer TAKE, or SKIP and the number.` },
  ];
}

export interface Decision { decision: "take" | "skip"; said: "take" | "skip"; named: Signal[]; grounded: Signal[] }

/**
 * The rules decided; the thought can only veto, and only with a reason the numbers support:
 * a SKIP (said before any TAKE) counts when it names a signal that IS a red flag in `v`. A bare
 * SKIP, or one naming no red flag present, is recorded as said and the trade is taken. (Box 0,
 * 2026-10-23: the 1B model answered the old "TAKE or SKIP?" with the bare last word "SKIP" on 6
 * of 6 decisions, so no entry was ever made; that veto carried no information.)
 */
export function parseDecision(text: string, v: Values = {}): Decision {
  const t = text.toUpperCase();
  const s = t.indexOf("SKIP"), k = t.indexOf("TAKE");
  const said = s >= 0 && (k < 0 || s < k) ? "skip" : "take";
  const low = text.toLowerCase();
  const named = RED_FLAGS.map((f) => f.signal).filter((sig) => low.includes(sig));
  const present = new Set(flagsIn(v));
  const grounded = named.filter((sig) => present.has(sig));
  return { decision: said === "skip" && grounded.length > 0 ? "skip" : "take", said, named, grounded };
}

// ------------------------------------------------------------ the lesson of a post-mortem

// The analyzer is the source of truth: the facts sentence (outcome, PnL, buckets) is written here,
// and the model only words what to change (or keep) for the bucket the analyzer named, answer
// first. Its line is checked against the facts and dropped when it contradicts them. Box 0,
// 2026-10-22/23 (fixtures/lessons.json): the old "one sentence starting with LESSON:" got lessons
// that called a trade that never filled "a significant loss", opened with "While I appreciate the
// thoroughness of your review..." and were cut off by the 60-token cap before saying anything.

const solStr = (lamports: string) => {
  const x = Number(BigInt(lamports)) / 1e9;
  return `${x > 0 ? "+" : x < 0 ? "-" : ""}${Math.abs(x)} SOL`;
};
const FIXABLE_BUCKETS: Bucket[] = ["INFORMATION", "THESIS", "EXECUTION", "SIZING", "RISK_MANAGEMENT"];

/** The bucket the lesson is about: the first fixable one the analyzer found at fault, if any. */
export function lessonFocus(r: Result): Bucket | null {
  return r.names.find((b) => FIXABLE_BUCKETS.includes(b)) ?? null;
}

/** The facts of the post-mortem in plain words, from the analyzer's result only. */
export function lessonFacts(r: Result): string {
  const variance = r.names.length === 1 && r.names[0] === "VARIANCE";
  const at = variance ? "no bucket at fault, the loss fits the stated win rate (VARIANCE)" : r.names.length ? `at fault: ${r.names.join(", ")}` : "no bucket at fault";
  switch (r.outcome) {
    case "no_fill": return `No fill: the entry was never filled, so nothing was bought and no SOL was gained or lost; ${at}.`;
    case "flat": return `Flat: the trade closed even, no SOL gained or lost; ${at}.`;
    case "win": return `Win: ${solStr(r.pnl_lamports)} on this trade; ${at}.`;
    default: return `Loss: ${solStr(r.pnl_lamports)} on this trade; ${at}.`;
  }
}

export function lessonPrompt(persona: string, r: Result): Msg[] {
  const focus = lessonFocus(r);
  const head = focus ? "FIX:" : "KEEP:";
  const ev = focus ? r.verdicts.find((x) => x.bucket === focus)?.evidence.slice(0, 2).join("; ") ?? "" : "";
  const ask = focus ? `the one thing to change next time about ${focus}` : "the one thing to keep doing next time";
  return [
    { role: "system", content: `You are ${persona}. You review your own practice trade. The facts are final: do not restate, judge or change them. Answer first: one line that starts with ${head} and gives, in at most 20 words, ${ask}. No preamble.` },
    { role: "user", content: `Facts: ${lessonFacts(r)}${focus ? `\nEvidence for ${focus}: ${ev}` : ""}\nReply with one line starting ${head}` },
  ];
}

/** The model's line: the text after FIX:/KEEP:/LESSON: (or the first line), filtered. */
export function parseLesson(text: string): string {
  const m = text.match(/\b(?:LESSON|FIX|KEEP)\b\s*:?\s*(.{1,280})/i);
  const s = (m ? m[1] : text).split("\n")[0].trim().replace(/^[*_"'\s]+|[*_"\s]+$/g, "").slice(0, 280);
  return checkOutput(s, { maxBytes: 300 }).ok ? s : "";
}

/**
 * Why a model line contradicts the analyzer's facts, or null when it does not: a trade that never
 * filled (or closed flat) called a gain or a loss, a win called a loss or a loss a win, a SOL
 * amount that is not this trade's PnL, a bucket named that the analyzer did not find, or a line
 * addressed to the reviewer instead of the trade.
 */
export function lessonContradiction(line: string, r: Result): string | null {
  const t = line.toLowerCase();
  if (/\b(your review|the review|thoroughness|i must (correct|disagree|admit|express))\b/.test(t)) return "it answers the review instead of naming a change";
  if ((r.outcome === "no_fill" || r.outcome === "flat") && /\b(loss|losses|lost|losing|lose|profit|profits|profitable|gain|gains|gained|won|win|wins|winning)\b/.test(t))
    return r.outcome === "no_fill" ? "it speaks of a gain or a loss, but the entry never filled" : "it speaks of a gain or a loss, but the trade closed flat";
  if (r.outcome === "win" && /\b(lost|losing trade|(a|the|this|significant|substantial|big|large|heavy) loss)\b/.test(t)) return "it calls a win a loss";
  if (r.outcome === "loss" && /\b(won|winning trade|made a profit|was profitable|(a|the|this) (win|profit|gain))\b/.test(t)) return "it calls a loss a win";
  const pnl = Math.abs(Number(BigInt(r.pnl_lamports)) / 1e9);
  for (const m of t.matchAll(/(\d+(?:\.\d+)?)\s*sol\b/g)) if (Math.abs(Number(m[1]) - pnl) > 1e-9) return `it names ${m[1]} SOL, but this trade's PnL is ${pnl} SOL`;
  for (const b of BUCKETS) if (new RegExp(`\\b${b.replace("_", "[ _]")}\\b`).test(line) && !r.names.includes(b)) return `it names ${b}, which the analyzer did not find`;
  return null;
}

export interface Lesson { lesson: string; facts: string; model_line: string; guard: string | null }

/** The committed lesson: the analyzer's facts, then the model's line when it passes the guard. */
export function composeLesson(text: string, r: Result): Lesson {
  const facts = lessonFacts(r);
  const line = parseLesson(text);
  const guard = line ? lessonContradiction(line, r) : "no usable line";
  const head = lessonFocus(r) ? "Next time" : "Keep";
  return { lesson: guard ? facts : `${facts} ${head}: ${line}`, facts, model_line: line, guard };
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
