// One practice agent (docs/STRATEGIES.md): it authors its strategy, watches real mainnet pools,
// commits every entry on Cadre BEFORE its pretend fill, exits on its stop, target or time,
// commits the exit, explains the trade in a post-mortem and commits it, and revises its strategy
// only when post-mortems name a fixable bucket. Pretend money, real prices, no mainnet tx.
import { existsSync, readFileSync, renameSync, writeFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import type { Cadre } from "../../trading/src/cadre.ts";
import type { Wallet } from "../../src/wallet.ts";
import { analyze, ANALYZER, BIT, BUCKET_ID, type Bucket, bucketsOf, FIXABLE, RED_FLAGS, type Result, type Snapshot, type TradeRecord, topFixable } from "./analyzer.ts";
import { MIN_FORWARD } from "./knowledge.ts";
import { type Deep, type Market, signals, SIGNALS_NEED_DEEP, type Watched } from "./market.ts";
import { priceE12 } from "./pump.ts";
import { call } from "./records.ts";
import { fillBuy, exitDue, markValue, MODEL, quote, type Quote, REASON_CODE } from "./sim.ts";
import { blobId, type Condition, conditionsHash, holds, inspected, type Signal, sizeFor, SPEC_FORMAT, type StrategySpec, specHash, validate, type Values } from "./spec.ts";
import type { BlobStore } from "./store.ts";
import { authorPrompt, decisionPrompt, fixesFor, fixPrompt, flagsIn, lessonPrompt, parseAuthor, parseDecision, parseFix, parseLesson, type Think } from "./thoughts.ts";

export interface Base {
  conditions: Condition[];
  size: StrategySpec["size"];
  max_risk_bps: number;
  max_slippage_bps: number;
  practice_balance_lamports: number;
}
/** `pin` (user agents, 2026-10-22): the creator's take profit / stop loss, fixed for every version
 * of the line (the authoring thought and later fixes cannot move them). Absent for house agents. */
export interface AgentCfg { handle: string; name: string; persona: string; base: Base; pin?: { stop_bps?: number; target_bps?: number } }

interface Hist { entry: number; version: number; conditions_hash: string; outcome: Result["outcome"]; buckets: number; pnl: string; used: boolean; missed: Signal[]; vol: number | null; sigma_hold_bps: number | null }
interface Open {
  stage: "committing" | "filled" | "exiting" | "explaining";
  pool: string;
  mint: string;
  entry_tx: string;
  entry: number;
  entry_height: number;
  snapshot_id: string;
  snapshot: Snapshot & Record<string, unknown>;
  quote: Quote;
  size: string;
  max_risk: string;
  fill: { ok: boolean; slot: number; out: string; price_e12: string; slippage_bps: number; reason: string };
  timing: TradeRecord["timing"];
  opened_ms: number;
  exit?: TradeRecord["exit"];
  exit_tx?: string;
  record_id?: string;
}
interface State { version: number; line: number; spec: StrategySpec | null; spec_id: string; open: Open | null; history: Hist[]; skips: Record<string, number> }

export interface AgentDeps {
  cadre: Cadre;
  contract: string;
  market: Market;
  store: BlobStore;
  think: Think;
  wallet: Wallet;
  stateFile: string;
  tickMs: number;
  analyzerHash: string;
  log: (e: string, x: Record<string, unknown>) => void;
  /** Reviseable after this many explained trades on the head version (default MIN_FORWARD). */
  reviseAfter?: number;
}

const big = (x: string | number | bigint) => BigInt(x);
const now = () => Date.now();

export class PracticeAgent {
  cfg: AgentCfg;
  d: AgentDeps;
  s: State;
  busy = false;
  constructor(cfg: AgentCfg, d: AgentDeps) {
    this.cfg = cfg;
    this.d = d;
    this.s = existsSync(d.stateFile) ? JSON.parse(readFileSync(d.stateFile, "utf8")) : { version: 0, line: 0, spec: null, spec_id: "", open: null, history: [], skips: {} };
  }
  save() {
    const tmp = `${this.d.stateFile}.tmp`;
    writeFileSync(tmp, JSON.stringify(this.s, null, 1));
    renameSync(tmp, this.d.stateFile);
  }
  log(e: string, x: Record<string, unknown> = {}) { this.d.log(e, { agent: this.cfg.handle, ...x }); }

  equity(): bigint {
    const start = big(this.s.spec?.practice_balance_lamports ?? this.cfg.base.practice_balance_lamports);
    return this.s.history.reduce((a, h) => a + big(h.pnl), start);
  }

  // ------------------------------------------------------------ Cadre commitments

  /** Simulate, send, wait until included; the receipt must be ok. Returns the call's return bytes. */
  async commit(args: Uint8Array, what: string, onSent?: (tx: string) => void): Promise<{ tx: string; height: number; ret: Buffer; sentMs: number; seenMs: number; status: string }> {
    const sent = await this.d.cadre.sendContract(this.d.wallet, { kind: "Call", contract: this.d.contract, value: 0n, gasLimit: 0n, args });
    onSent?.(sent.txHash);
    const seen = await this.seen(sent.txHash);
    const inc = await this.d.cadre.waitIncluded(sent.txHash, 900_000, 1_000);
    const r = inc.receipt;
    if (!r || r.ok === false || (r.status && r.status !== "ok")) throw new Error(`${what} ${sent.txHash} failed on chain: ${r?.error ? Buffer.from(r.error, "hex").toString("utf8") || r.error : "no receipt"}`);
    this.log("committed", { what, tx: sent.txHash, height: inc.height, fee: sent.fee.toString(), seen: seen.status, seen_ms: seen.ms - sent.sentAt });
    return { tx: sent.txHash, height: inc.height, ret: Buffer.from(r.return ?? "", "hex"), sentMs: sent.sentAt, seenMs: seen.ms, status: seen.status };
  }

  /** First moment the tx is visible as preconfirmed (a weak block on the tip lists it) or included. */
  async seen(tx: string, timeoutMs = 180_000): Promise<{ status: string; ms: number }> {
    const end = now() + timeoutMs;
    while (now() < end) {
      const st = await this.d.cadre.rpc.call("getTxStatus", { tx_hash: tx }).catch(() => null);
      if (st && ["preconfirmed", "included", "final"].includes(st.status)) return { status: st.status, ms: now() };
      await new Promise((r) => setTimeout(r, 500));
    }
    throw new Error(`tx ${tx} not seen in ${timeoutMs} ms`);
  }

  // ------------------------------------------------------------ authoring

  async ensureVersion(): Promise<void> {
    if (this.s.version) return;
    const t = await this.d.think(authorPrompt(this.cfg.persona, this.cfg.base), 48);
    const p = parseAuthor(t.text);
    const spec: StrategySpec = {
      format: SPEC_FORMAT, name: `${this.cfg.handle} v1`, author: this.d.wallet.address, parent: null, venue: "pumpswap", mode: "practice",
      practice_balance_lamports: this.cfg.base.practice_balance_lamports, conditions: this.cfg.base.conditions, size: this.cfg.base.size,
      stop_bps: p.stop_bps, target_bps: p.target_bps, max_hold_s: p.max_hold_s, max_risk_bps: this.cfg.base.max_risk_bps, max_slippage_bps: this.cfg.base.max_slippage_bps,
      edge: { win_rate: p.win_rate, avg_win_bps: p.target_bps, avg_loss_bps: p.stop_bps }, rationale: p.rationale, change: "", authored_by: t.requestId,
    };
    await this.publish(this.pinned(spec), 0, 0, [], t.requestId);
  }

  /** The creator's pinned exits, when set (AgentCfg.pin), override what a thought or a fix chose. */
  pinned(spec: StrategySpec): StrategySpec {
    const p = this.cfg.pin;
    if (!p) return spec;
    const stop = p.stop_bps ?? spec.stop_bps, target = p.target_bps ?? spec.target_bps;
    return { ...spec, stop_bps: stop, target_bps: target, edge: { ...spec.edge, avg_win_bps: target, avg_loss_bps: stop } };
  }

  async publish(spec: StrategySpec, parent: number, claimed: number, motives: number[], thoughtRef: string): Promise<void> {
    const errs = validate(spec);
    if (errs.length) throw new Error(`spec out of bounds: ${errs.join(", ")}`);
    const put = await this.d.store.put(spec);
    if (put.id !== specHash(spec)) throw new Error("spec hash mismatch");
    const c = await this.commit(call.publish(put.id, parent, `blob:${put.id}`, thoughtRef, claimed, motives), "publish");
    const id = Number(c.ret.readBigUInt64LE(0));
    this.s.version = id;
    if (!parent) this.s.line = id;
    this.s.spec = spec;
    this.s.spec_id = put.id;
    for (const h of this.s.history) if (motives.includes(h.entry)) h.used = true;
    this.save();
    this.log("version", { id, parent, claimed, motives, hash: put.id, api: put.api, node: put.node, change: spec.change || "first version" });
  }

  // ------------------------------------------------------------ one tick

  async tick(): Promise<void> {
    if (this.busy) return;
    this.busy = true;
    try {
      await this.ensureVersion();
      if (this.s.open) await this.manage();
      else await this.scan();
    } catch (e) {
      this.log("error", { message: (e as Error).message.slice(0, 400) });
    } finally {
      this.busy = false;
    }
  }

  candidates(): { w: Watched; v: Values }[] {
    const spec = this.s.spec!;
    const out: { w: Watched; v: Values }[] = [];
    for (const w of this.d.market.pools.values()) {
      if ((this.s.skips[w.pool.address] ?? 0) > now()) continue;
      const last = w.obs.at(-1);
      if (!last) continue;
      const v = signals(w, this.d.market.fees(w, { base: big(last.base), quote: big(last.quote) }), null);
      // conditions on deep signals are checked after the deep read
      const shallow = { ...spec, conditions: spec.conditions.filter((c) => !SIGNALS_NEED_DEEP.includes(c.signal)) };
      if (shallow.conditions.length === 0 || holds(shallow, v).ok) out.push({ w, v });
    }
    return out.sort((a, b) => (b.v.pool_sol ?? 0) - (a.v.pool_sol ?? 0));
  }

  async scan(): Promise<void> {
    const spec = this.s.spec!;
    for (const { w } of this.candidates().slice(0, 3)) {
      const deep = await this.d.market.deep(w);
      const last = w.obs.at(-1)!;
      const fees = this.d.market.fees(w, { base: big(last.base), quote: big(last.quote) });
      const v0 = signals(w, fees, deep);
      if (!holds(spec, v0).ok) continue;
      await this.enter(w, v0, last.slot);
      return;
    }
  }

  async enter(w: Watched, v0: Values, slot0: number): Promise<void> {
    const spec = this.s.spec!;
    const equity = this.equity();
    const sz0 = sizeFor(spec, equity, big(w.obs.at(-1)!.quote));
    const t = await this.d.think(decisionPrompt(this.cfg.persona, spec, v0, sz0.size), 24);
    const decisionMs = now();
    const dec = parseDecision(t.text, v0);
    if (dec.decision === "skip") {
      this.s.skips[w.pool.address] = now() + 10 * 60_000;
      this.save();
      this.log("skip", { pool: w.pool.address, request: t.requestId, said: t.text.slice(0, 120), flags: dec.grounded });
      return;
    }
    if (dec.said === "skip") this.log("veto_unsupported", { pool: w.pool.address, request: t.requestId, said: t.text.slice(0, 120), named: dec.named, flags_present: flagsIn(v0) });
    // the decision slot: a fresh read after the thought, rules checked again on it
    const r = await this.d.market.readPool(w.pool.address);
    w.obs.push({ slot: r.slot, t: r.t, base: r.base.toString(), quote: r.quote.toString(), dev: w.obs.at(-1)?.dev ?? null });
    const deep = await this.d.market.deep(w);
    const fees = this.d.market.fees(w, r);
    const v = signals(w, fees, deep, r.t);
    const h = holds(spec, v);
    if (!h.ok) {
      this.log("condition_gone", { pool: w.pool.address, failed: h.failed, request: t.requestId });
      return;
    }
    const sz = sizeFor(spec, equity, r.quote);
    if (sz.size <= 0n) return;
    const q = quote(r, fees, sz.size, r.slot);
    const snap = {
      format: "cadre-practice-snapshot-v1",
      agent: this.d.wallet.address,
      version: this.s.version,
      version_hash: this.s.spec_id,
      mode: "practice: pretend balance, real Solana mainnet state, no mainnet transaction",
      pool: { address: w.pool.address, mint: w.pool.baseMint, base_vault: w.pool.baseVault, quote_vault: w.pool.quoteVault, lp_mint: w.pool.lpMint, coin_creator: w.pool.coinCreator, token_program: w.tokenProgram, supply: w.supply.toString(), decimals: w.decimals },
      slot: r.slot,
      read_at_ms: r.t,
      reserves: { base: r.base.toString(), quote: r.quote.toString() },
      fees,
      available: v,
      inspected: [...inspected(spec)].sort(),
      conditions_hash: conditionsHash(spec.conditions),
      conditions: spec.conditions.map((c) => ({ ...c, value_at_slot: v[c.signal] ?? null })),
      holders: deep.holders,
      lp: deep.lp,
      lp_read: deep.lpRead,
      deep_slot: deep.slot,
      tick_ms: this.d.tickMs,
      observations: w.obs.filter((o) => o.t >= r.t - 3600_000),
      thought_inputs: { slot: slot0, values: v0 },
      decision: { request_id: t.requestId, text: t.text.slice(0, 400), said: dec.said, named: dec.named, grounded: dec.grounded, rule: "a SKIP counts only when it names a red flag present in the numbers" },
      equity_lamports: equity.toString(),
      size: { lamports: sz.size.toString(), bound: sz.bound, max_risk_lamports: sz.maxRisk.toString() },
      quote: q,
      latency_model: MODEL,
    };
    const put = await this.d.store.put(snap);
    // commit BEFORE the fill: the fill is priced only once the commitment is visible on Cadre
    const c = await this.commit(call.enter({ version: this.s.version, snapshot: put.id, size: sz.size, priceE12: big(q.price_e12), quoteOut: big(q.out), stopBps: spec.stop_bps, targetBps: spec.target_bps, maxRisk: sz.maxRisk, decisionSlot: r.slot, decisionRef: t.requestId }), "enter");
    const entry = Number(c.ret.readBigUInt64LE(0));
    const fr = await this.d.market.readPool(w.pool.address, r.slot + MODEL.latency_slots);
    const fill = fillBuy(q, fr, this.d.market.fees(w, fr), fr.slot, spec.max_slippage_bps);
    const fillMs = now();
    this.s.open = {
      stage: "filled", pool: w.pool.address, mint: w.pool.baseMint, entry_tx: c.tx, entry, entry_height: c.height, snapshot_id: put.id, snapshot: snap as any, quote: q, size: sz.size.toString(), max_risk: sz.maxRisk.toString(), fill,
      timing: { decision_ms: decisionMs, commit_seen_ms: c.seenMs, fill_ms: fillMs, latency_slots_model: MODEL.latency_slots }, opened_ms: fillMs,
    };
    this.save();
    this.log("entered", { entry, pool: w.pool.address, mint: w.pool.baseMint, size: sz.size.toString(), quote_out: q.out, fill_ok: fill.ok, fill_slippage_bps: fill.slippage_bps, commit_seen: c.status, commit_wait_ms: c.seenMs - decisionMs, snapshot: put.id, api: put.api });
  }

  async manage(): Promise<void> {
    const o = this.s.open!;
    const spec = this.s.spec!;
    if (o.stage === "filled") {
      if (!o.fill.ok) {
        o.exit = null;
        o.stage = "exiting";
      } else {
        const w = this.d.market.pools.get(o.pool);
        const last = w?.obs.at(-1);
        if (!w || !last) return;
        const rr = { base: big(last.base), quote: big(last.quote) };
        const value = markValue(rr, this.d.market.fees(w, rr), big(o.fill.out));
        const due = exitDue(value, big(o.size), spec.stop_bps, spec.target_bps, now() - o.opened_ms, spec.max_hold_s);
        if (!due) return;
        const xr = await this.d.market.readPool(o.pool, last.slot + MODEL.latency_slots);
        const proceeds = markValue(xr, this.d.market.fees(w, xr), big(o.fill.out));
        const slip = value > 0n ? Number(((value - proceeds) * 10_000n) / value) : 0;
        o.exit = { reason: due, trigger_slot: last.slot, trigger_value: value.toString(), slot: xr.slot, proceeds: proceeds.toString(), slippage_bps: Math.max(0, slip) };
        o.stage = "exiting";
        this.log("exit_due", { entry: o.entry, reason: due, value: value.toString(), proceeds: proceeds.toString() });
      }
      this.save();
    }
    if (o.stage === "exiting") {
      const height = await this.d.cadre.height();
      if (height <= o.entry_height) return; // the contract takes the exit only in a later block
      const rec = {
        format: "cadre-practice-trade-v1", agent: this.d.wallet.address, entry: o.entry, entry_tx: o.entry_tx, version: this.s.version, snapshot: o.snapshot_id,
        size: o.size, max_risk: o.max_risk, quote: o.quote, fill: o.fill, exit: o.exit ?? null, timing: o.timing,
        hold: (this.d.market.pools.get(o.pool)?.obs ?? []).filter((x) => x.t >= o.opened_ms - this.d.tickMs).map((x) => ({ slot: x.slot, t: x.t, base: x.base, quote: x.quote })),
        simulated: "fill and exit priced on real mainnet reserves with PumpSwap's fee tier; latency modelled (latency_model in the snapshot); our own trade is not in the pool",
      };
      const put = await this.d.store.put(rec);
      o.record_id = put.id;
      const ex = o.exit;
      const args = ex
        ? call.exit({ entry: o.entry, fillSlot: o.fill.slot, fillOut: big(o.fill.out), fillPriceE12: big(o.fill.price_e12), exitSlot: ex.slot, reason: REASON_CODE[ex.reason], proceeds: big(ex.proceeds), record: put.id })
        : call.exit({ entry: o.entry, fillSlot: 0, fillOut: 0n, fillPriceE12: 0n, exitSlot: 0, reason: REASON_CODE.no_fill, proceeds: big(o.size), record: put.id });
      const c = await this.commit(args, "exit");
      o.exit_tx = c.tx;
      o.stage = "explaining";
      this.save();
    }
    if (o.stage === "explaining") await this.explain();
  }

  async explain(): Promise<void> {
    const o = this.s.open!;
    const spec = this.s.spec!;
    const ch = o.snapshot.conditions_hash;
    const sameSet = this.s.history.filter((h) => h.conditions_hash === ch);
    const mine = this.s.history.filter((h) => h.version === this.s.version && h.outcome !== "no_fill");
    const rec: TradeRecord = { size: o.size, max_risk: o.max_risk, quote: o.quote, fill: o.fill, exit: o.exit ?? null, timing: o.timing };
    const r = analyze(spec, o.snapshot, rec, { outcomes: sameSet.map((h) => h.outcome), version_trades: mine.length, version_losses: mine.filter((h) => h.outcome === "loss").length });
    const t = await this.d.think(lessonPrompt(this.cfg.persona, r), 60);
    const lesson = parseLesson(t.text);
    const blob = { ...r, agent: this.d.wallet.address, entry: o.entry, version: this.s.version, version_hash: this.s.spec_id, snapshot: o.snapshot_id, record: o.record_id, analyzer_hash: this.d.analyzerHash, lesson, lesson_request: t.requestId, lesson_text_raw: t.text.slice(0, 400) };
    const put = await this.d.store.put(blob);
    await this.commit(call.postmortem(o.entry, put.id, r.buckets, this.d.analyzerHash, t.requestId), "postmortem");
    const info = r.verdicts.find((x) => x.bucket === "INFORMATION")!;
    const missed = RED_FLAGS.filter((f) => info.evidence.some((e) => e.startsWith(`${f.signal} =`) && e.includes("not inspected"))).map((f) => f.signal);
    const vol = (o.snapshot.available as Values).vol_15m ?? null;
    const sigma = vol !== null ? vol * Math.sqrt(Math.max(1, (spec.max_hold_s * 1000) / this.d.tickMs)) * 10_000 : null;
    this.s.history.push({ entry: o.entry, version: this.s.version, conditions_hash: ch, outcome: r.outcome, buckets: r.buckets, pnl: r.pnl_lamports, used: false, missed, vol, sigma_hold_bps: sigma });
    this.s.open = null;
    this.save();
    this.log("postmortem", { entry: o.entry, outcome: r.outcome, pnl: r.pnl_lamports, buckets: r.names, findings: r.findings, lesson, result: put.id });
    await this.maybeRevise(r);
  }

  /** The learning rule: only after a post-mortem names a fixable bucket, never after VARIANCE alone. */
  async maybeRevise(last: Result): Promise<void> {
    if (!FIXABLE.some((b) => last.buckets & BIT[b])) return;
    const head = this.s.history.filter((h) => h.version === this.s.version);
    if (head.length < (this.d.reviseAfter ?? MIN_FORWARD)) return;
    const unused = this.s.history.filter((h) => !h.used);
    const b = topFixable(head.filter((h) => !h.used).map((h) => h.buckets));
    if (!b) return;
    const cited = unused.filter((h) => h.buckets & BIT[b]).slice(-8);
    if (cited.length === 0) return;
    const spec = this.s.spec!;
    const decided = head.filter((h) => h.outcome !== "no_fill");
    const vols = head.map((h) => h.vol).filter((x): x is number => x !== null).sort((a, c) => a - c);
    const sig = head.map((h) => h.sigma_hold_bps).filter((x): x is number => x !== null).sort((a, c) => a - c);
    const missedCount = new Map<Signal, number>();
    for (const h of cited) for (const m of h.missed) missedCount.set(m, (missedCount.get(m) ?? 0) + 1);
    const fixes = fixesFor(b, spec, {
      missed: [...missedCount.entries()].sort((a, c) => c[1] - a[1]).map((x) => x[0]),
      observedWinRate: decided.length ? decided.filter((h) => h.outcome === "win").length / decided.length : null,
      sigmaHoldBps: sig.length ? sig[Math.floor(sig.length / 2)] : null,
      medianVol: vols.length ? vols[Math.floor(vols.length / 2)] : null,
    });
    // a fix that only moves pinned exits changes nothing: drop it before paying for the choice
    const same = JSON.stringify(this.pinned(spec));
    const real = fixes.filter((f) => JSON.stringify(this.pinned(f.apply(spec))) !== same);
    fixes.splice(0, fixes.length, ...real);
    if (fixes.length === 0) return;
    const t = await this.d.think(fixPrompt(this.cfg.persona, b, fixes, cited.length), 24);
    const f = fixes[parseFix(t.text, fixes.length)];
    const n = Number(this.s.spec!.name.match(/v(\d+)$/)?.[1] ?? 1) + 1;
    const next: StrategySpec = { ...this.pinned(f.apply(spec)), name: `${this.cfg.handle} v${n}`, parent: this.s.spec_id, change: `${b}: ${f.text} (post-mortems ${cited.map((h) => h.entry).join(", ")})`, authored_by: t.requestId };
    await this.publish(next, this.s.version, BUCKET_ID[b], cited.map((h) => h.entry), t.requestId);
  }
}

/** blake3 of the analyzer's source file: committed with every post-mortem, so a rerun picks the same code. */
export const analyzerHash = () => blobId(readFileSync(fileURLToPath(new URL("./analyzer.ts", import.meta.url))));
export { ANALYZER, bucketsOf, priceE12 };
export type { Bucket, Deep };
