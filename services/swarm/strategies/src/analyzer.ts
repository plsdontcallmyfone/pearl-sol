// cadre-postmortem-v1: the deterministic half of a post-mortem (docs/STRATEGIES.md 4). Given the
// committed strategy, the committed entry snapshot (the state available at the decision slot and
// the subset the strategy inspected), the trade record and the agent's committed history, it puts
// the outcome in buckets with evidence. Anyone holding the blobs reruns it and gets the same bits;
// the blake3 of this file is committed with every post-mortem as the analyzer hash. A paid Cadre
// inference then reads the result and writes the lesson; it cannot change a bucket.
//
// Every threshold below is a proposed default (a design choice, not a measured figure).
import type { Signal, StrategySpec, Values } from "./spec.ts";

export const ANALYZER = "cadre-postmortem-v1";

export const BUCKETS = ["INFORMATION", "THESIS", "EXECUTION", "SIZING", "RISK_MANAGEMENT", "LEVERAGE", "VARIANCE"] as const;
export type Bucket = (typeof BUCKETS)[number];
export const BIT: Record<Bucket, number> = { INFORMATION: 1, THESIS: 2, EXECUTION: 4, SIZING: 8, RISK_MANAGEMENT: 16, LEVERAGE: 32, VARIANCE: 64 };
/** Bucket id as the contract's `claimed` field counts them (1-based). */
export const BUCKET_ID: Record<Bucket, number> = { INFORMATION: 1, THESIS: 2, EXECUTION: 3, SIZING: 4, RISK_MANAGEMENT: 5, LEVERAGE: 6, VARIANCE: 7 };
export const FIXABLE: Bucket[] = ["INFORMATION", "THESIS", "EXECUTION", "SIZING", "RISK_MANAGEMENT"];
export const bucketsOf = (bits: number): Bucket[] => BUCKETS.filter((b) => bits & BIT[b]);

/** Red flags the INFORMATION bucket checks the available set against (proposed defaults). */
export const RED_FLAGS: { signal: Signal; op: ">" | "<"; value: number; why: string }[] = [
  { signal: "top10_share", op: ">", value: 0.4, why: "the 10 largest holders own more than 40% of supply" },
  { signal: "top1_share", op: ">", value: 0.15, why: "one holder owns more than 15% of supply" },
  { signal: "dev_share", op: ">", value: 0.05, why: "the coin creator holds more than 5% of supply" },
  { signal: "dev_sold_15m", op: ">", value: 0, why: "the coin creator sold in the last 15 minutes" },
  { signal: "pool_sol", op: "<", value: 20, why: "less than 20 SOL on the pool's quote side" },
  { signal: "ret_15m", op: ">", value: 0.5, why: "the price rose more than 50% in 15 minutes (late entry)" },
  { signal: "ret_60m", op: "<", value: -0.4, why: "the price fell more than 40% in an hour" },
  { signal: "flow_15m", op: "<", value: -0.05, why: "the pool lost more than 5% of its SOL in 15 minutes (net selling)" },
  { signal: "vol_15m", op: ">", value: 0.05, why: "observation-to-observation moves above 5% (one standard deviation)" },
  { signal: "lp_wallet_share", op: ">", value: 0.1, why: "plain wallets hold more than 10% of LP (liquidity can be pulled)" },
];
export const THESIS_MIN_N = 5;
export const ALPHA = 0.05;
/** A stop is "inside the noise" when its distance is below this many standard deviations of the hold. */
export const STOP_SIGMA = 1;
/** A stop exit counts as a gap when the sale came in this share of the stop distance below the stop. */
export const GAP_SHARE = 0.25;

export interface Snapshot {
  slot: number;
  available: Values;
  inspected: Signal[];
  conditions_hash: string;
  tick_ms: number;
}

export interface TradeRecord {
  size: string;
  max_risk: string;
  quote: { out: string; impact_bps: number; slot: number };
  fill: { ok: boolean; slot: number; out: string; slippage_bps: number; reason: string };
  exit: null | { reason: "stop" | "target" | "time"; trigger_slot: number; trigger_value: string; slot: number; proceeds: string; slippage_bps: number };
  timing: { decision_ms: number; commit_seen_ms: number; fill_ms: number; latency_slots_model: number };
}

/** Past explained trades of this agent with the same condition set, oldest first (this one excluded). */
export interface History { outcomes: ("win" | "loss" | "flat" | "no_fill")[]; version_losses: number; version_trades: number }

export interface Verdict { bucket: Bucket; fault: boolean; status: "fault" | "pass" | "not_applicable" | "not_enough_data"; evidence: string[] }
export interface Result {
  format: "cadre-postmortem-v1";
  analyzer: string;
  outcome: "win" | "loss" | "flat" | "no_fill";
  pnl_lamports: string;
  buckets: number;
  names: Bucket[];
  verdicts: Verdict[];
  findings: string[];
}

// ------------------------------------------------------------ binomial tails (exact)

function logChoose(n: number, k: number): number {
  let s = 0;
  for (let i = 1; i <= k; i++) s += Math.log((n - k + i) / i);
  return s;
}
export function binomPmf(k: number, n: number, p: number): number {
  if (p <= 0) return k === 0 ? 1 : 0;
  if (p >= 1) return k === n ? 1 : 0;
  return Math.exp(logChoose(n, k) + k * Math.log(p) + (n - k) * Math.log(1 - p));
}
/** P(X <= k), X ~ Binomial(n, p). */
export function binomCdf(k: number, n: number, p: number): number {
  let s = 0;
  for (let i = 0; i <= k; i++) s += binomPmf(i, n, p);
  return Math.min(1, s);
}
/** P(X >= k). */
export const binomSf = (k: number, n: number, p: number) => (k <= 0 ? 1 : Math.min(1, 1 - binomCdf(k - 1, n, p)));

const pct = (x: number) => `${(x * 100).toFixed(2)}%`;
const fmt = (x: number | null | undefined) => (x === null || x === undefined ? "unavailable" : Math.abs(x) >= 100 ? x.toFixed(1) : Number(x.toPrecision(4)).toString());

export function analyze(spec: StrategySpec, snap: Snapshot, rec: TradeRecord, hist: History): Result {
  const size = BigInt(rec.size);
  const proceeds = rec.fill.ok && rec.exit ? BigInt(rec.exit.proceeds) : size;
  const pnl = proceeds - size;
  const outcome: Result["outcome"] = !rec.fill.ok ? "no_fill" : pnl > 0n ? "win" : pnl < 0n ? "loss" : "flat";
  const v: Verdict[] = [];
  const insp = new Set(snap.inspected);

  // INFORMATION: red flags in the available set that the strategy did not inspect, or inspected
  // with a condition that let them through.
  {
    const ev: string[] = [];
    let fault = false;
    for (const f of RED_FLAGS) {
      const x = snap.available[f.signal];
      if (x === null || x === undefined) {
        if (!insp.has(f.signal)) continue;
        ev.push(`${f.signal}: inspected but unavailable at slot ${snap.slot}`);
        continue;
      }
      const red = f.op === ">" ? x > f.value : x < f.value;
      if (!red) continue;
      fault = true;
      ev.push(insp.has(f.signal)
        ? `${f.signal} = ${fmt(x)} was inspected, but the condition let it through: ${f.why}`
        : `${f.signal} = ${fmt(x)} was available at slot ${snap.slot} and not inspected: ${f.why}`);
    }
    const notInspected = Object.keys(snap.available).filter((k) => !insp.has(k as Signal) && snap.available[k as Signal] !== null);
    ev.push(`inspected ${insp.size} of ${Object.keys(snap.available).length} available signals; not inspected: ${notInspected.join(", ") || "none"}`);
    v.push({ bucket: "INFORMATION", fault, status: fault ? "fault" : "pass", evidence: ev });
  }

  // THESIS: the condition set's base rate over its past firings (committed trades) against the stated win rate.
  const firings = [...hist.outcomes, outcome].filter((o) => o !== "no_fill");
  const wins = firings.filter((o) => o === "win").length;
  {
    const n = firings.length;
    const p = spec.edge.win_rate;
    if (n < THESIS_MIN_N) {
      v.push({ bucket: "THESIS", fault: false, status: "not_enough_data", evidence: [`${wins} wins in ${n} firings of this condition set; the base rate is judged from ${THESIS_MIN_N} firings`, `stated win rate ${pct(p)}`] });
    } else {
      const pv = binomCdf(wins, n, p);
      const fault = pv < ALPHA;
      v.push({ bucket: "THESIS", fault, status: fault ? "fault" : "pass", evidence: [`${wins} wins in ${n} firings (${pct(wins / n)}) against a stated ${pct(p)}`, `P(this few wins | stated rate) = ${pv.toFixed(4)}${fault ? `, below ${ALPHA}: the edge as stated is not there` : ""}`] });
    }
  }

  // EXECUTION: no fill, slippage at the exit beyond the strategy's limit.
  {
    const ev = [`entry fill ${rec.fill.slippage_bps} bps against the quote at slot ${rec.quote.slot}, filled at slot ${rec.fill.slot} (${rec.fill.slot - rec.quote.slot} slots later; model ${rec.timing.latency_slots_model})`,
      `decision to commit visible ${Math.round(rec.timing.commit_seen_ms - rec.timing.decision_ms)} ms, decision to fill ${Math.round(rec.timing.fill_ms - rec.timing.decision_ms)} ms (the commit wait is a practice-mode cost: the fill waits for the commitment)`];
    let fault = false;
    if (!rec.fill.ok) {
      fault = true;
      ev.push(`no fill: ${rec.fill.reason}`);
    }
    if (rec.exit) {
      ev.push(`exit sale ${rec.exit.slippage_bps} bps below the value at the trigger (slot ${rec.exit.trigger_slot} -> ${rec.exit.slot})`);
      if (rec.exit.slippage_bps > spec.max_slippage_bps) {
        fault = true;
        ev.push(`exit slippage above max_slippage_bps ${spec.max_slippage_bps}`);
      }
    }
    v.push({ bucket: "EXECUTION", fault, status: fault ? "fault" : "pass", evidence: ev });
  }

  // SIZING: the loss against the committed max risk; the size's own impact on the pool.
  {
    const maxRisk = BigInt(rec.max_risk);
    const ev = [`size ${rec.size} lamports, max risk ${rec.max_risk}, entry impact ${rec.quote.impact_bps} bps`];
    let fault = false;
    if (pnl < 0n && -pnl > maxRisk) {
      fault = true;
      ev.push(`lost ${-pnl} lamports, ${Number(((-pnl - maxRisk) * 10_000n) / (maxRisk === 0n ? 1n : maxRisk)) / 100}% above the stated max risk`);
    } else if (pnl < 0n) ev.push(`lost ${-pnl} lamports, within max risk`);
    if (rec.quote.impact_bps > spec.max_slippage_bps) {
      fault = true;
      ev.push(`the size alone moved the price ${rec.quote.impact_bps} bps, above max_slippage_bps ${spec.max_slippage_bps}: too large for this pool`);
    }
    v.push({ bucket: "SIZING", fault, status: fault ? "fault" : "pass", evidence: ev });
  }

  // RISK_MANAGEMENT: stop placement against the volatility of the hold; gaps through the stop.
  {
    const vol = snap.available.vol_15m;
    const ev: string[] = [];
    let fault = false;
    let status: Verdict["status"] = "pass";
    if (vol === null || vol === undefined) {
      status = "not_enough_data";
      ev.push("vol_15m unavailable at the decision slot (fewer than 6 observations in 15 minutes)");
    } else {
      const ticks = Math.max(1, (spec.max_hold_s * 1000) / snap.tick_ms);
      const sigma = vol * Math.sqrt(ticks);
      const stop = spec.stop_bps / 10_000;
      ev.push(`stop ${pct(stop)}, one standard deviation over the ${spec.max_hold_s} s hold ${pct(sigma)} (vol_15m ${fmt(vol)} per ${snap.tick_ms / 1000} s observation)`);
      if (rec.exit?.reason === "stop" && stop < STOP_SIGMA * sigma) {
        fault = true;
        ev.push(`stopped out with the stop inside ${STOP_SIGMA} standard deviation of the hold: noise can trigger it`);
      }
    }
    if (rec.exit?.reason === "stop") {
      const stopValue = (size * BigInt(10_000 - spec.stop_bps)) / 10_000n;
      const gap = stopValue - BigInt(rec.exit.proceeds);
      const dist = size - stopValue;
      if (gap > 0n && Number(gap) > GAP_SHARE * Number(dist)) {
        fault = true;
        ev.push(`gapped through the stop: sold ${gap} lamports below the stop level (more than ${GAP_SHARE * 100}% of the stop distance); a ${snap.tick_ms / 1000} s check cannot stop a gap`);
      }
    }
    v.push({ bucket: "RISK_MANAGEMENT", fault, status: fault ? "fault" : status, evidence: ev });
  }

  v.push({ bucket: "LEVERAGE", fault: false, status: "not_applicable", evidence: ["spot only: no leverage is used, so none can be at fault"] });

  const faults = v.filter((x) => x.fault);
  let bits = faults.reduce((a, x) => a | BIT[x.bucket], 0);
  const findings: string[] = [];
  // VARIANCE: only a loss with every bucket passing, and only while this version's losses are
  // consistent with its stated loss rate; otherwise the edge is wrong (THESIS).
  if (outcome === "loss" && faults.length === 0) {
    const n = hist.version_trades + 1;
    const losses = hist.version_losses + 1;
    const q = 1 - spec.edge.win_rate;
    const pv = binomSf(losses, n, q);
    if (pv >= ALPHA) {
      bits = BIT.VARIANCE;
      v.push({ bucket: "VARIANCE", fault: false, status: "pass", evidence: [`every bucket passed; ${losses} losses in ${n} trades of this version against a stated loss rate ${pct(q)} (P(this many or more) = ${pv.toFixed(4)})`, "a necessary loss that comes with the system"] });
    } else {
      bits = BIT.THESIS;
      const t = v.find((x) => x.bucket === "THESIS")!;
      t.fault = true;
      t.status = "fault";
      t.evidence.push(`${losses} losses in ${n} trades of this version against a stated loss rate ${pct(q)}: P(this many or more) = ${pv.toFixed(4)}, below ${ALPHA}`);
      v.push({ bucket: "VARIANCE", fault: false, status: "not_applicable", evidence: ["losses exceed what the stated loss rate allows: THESIS, not variance"] });
    }
  } else {
    v.push({ bucket: "VARIANCE", fault: false, status: "not_applicable", evidence: [outcome === "loss" ? "a fault bucket applies" : `outcome ${outcome}`] });
  }
  if (outcome === "win" && bits !== 0) findings.push(`lucky win: it made money although ${bucketsOf(bits).join(", ")} ${bucketsOf(bits).length > 1 ? "are" : "is"} at fault`);
  if (outcome === "win" && bits === 0) findings.push("clean win: every bucket passed");
  return { format: "cadre-postmortem-v1", analyzer: ANALYZER, outcome, pnl_lamports: pnl.toString(), buckets: bits, names: bucketsOf(bits), verdicts: v, findings };
}

/** The fixable bucket a revision should claim: the most frequent fault among the cited post-mortems (ties: BUCKETS order). */
export function topFixable(bitsList: number[]): Bucket | null {
  let best: Bucket | null = null;
  let n = 0;
  for (const b of FIXABLE) {
    const c = bitsList.filter((x) => x & BIT[b]).length;
    if (c > n) { best = b; n = c; }
  }
  return best;
}
