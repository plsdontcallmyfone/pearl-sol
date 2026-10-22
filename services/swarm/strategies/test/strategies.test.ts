import { strict as assert } from "node:assert";
import { mkdtempSync, writeFileSync, mkdirSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { test } from "node:test";
import { walletFromSeed } from "../../src/wallet.ts";
import { PracticeAgent } from "../src/agent.ts";
import { analyze, BIT, binomCdf, type Snapshot, type TradeRecord } from "../src/analyzer.ts";
import { fisherLower, knowledgeOf, MIN_FORWARD, records } from "../src/knowledge.ts";
import { mainnetUrl, MAINNET_GENESIS, ReadOnlySolana } from "../src/mainnet.ts";
import { signals, type Watched } from "../src/market.ts";
import { buyOut, decodeFeeConfig, feesFor, sellOut, totalBps } from "../src/pump.ts";
import { call, decodeTrade, decodeVersion, type TradeRec, type VersionRec } from "../src/records.ts";
import { exitDue, fillBuy, quote } from "../src/sim.ts";
import { canonicalJson, conditionsHash, holds, sizeFor, SPEC_FORMAT, specHash, type StrategySpec, validate } from "../src/spec.ts";
import { BlobStore } from "../src/store.ts";
import { fixesFor, parseAuthor, parseDecision, parseFix, parseLesson } from "../src/thoughts.ts";

const spec = (o: Partial<StrategySpec> = {}): StrategySpec => ({
  format: SPEC_FORMAT, name: "t v1", author: "A", parent: null, venue: "pumpswap", mode: "practice", practice_balance_lamports: 1_000_000_000,
  conditions: [{ signal: "pool_sol", op: ">=", value: 60 }, { signal: "ret_15m", op: ">=", value: 0.03 }],
  size: { equity_fraction_bps: 1000, max_lamports: 100_000_000, max_pool_share_bps: 20 }, stop_bps: 800, target_bps: 1500, max_hold_s: 1200, max_risk_bps: 100, max_slippage_bps: 150,
  edge: { win_rate: 0.45, avg_win_bps: 1500, avg_loss_bps: 800 }, rationale: "", change: "", authored_by: "req_x", ...o,
});

// ------------------------------------------------------------ mainnet guard

test("mainnet client: only read methods, never a write, and it checks the genesis hash", async () => {
  let fetched = 0;
  const f = (async (_u: string, init: any) => { fetched++; const b = JSON.parse(init.body); return new Response(JSON.stringify({ jsonrpc: "2.0", id: b.id, result: b.method === "getGenesisHash" ? "EtWTRABZaYq6iMfeYKouRu166VU2xqa1wcaWoxPkrZBG" : null })); }) as any;
  const sol = new ReadOnlySolana("https://example.invalid/?api-key=SECRET", f);
  for (const m of ["sendTransaction", "simulateTransaction", "requestAirdrop", "sendRawTransaction"]) await assert.rejects(sol.call(m, []), /refused: .* is not a read method/);
  assert.equal(fetched, 0, "a refused method never reaches the network");
  await assert.rejects(sol.ensureMainnet(), /not Solana mainnet-beta/);
  const ok = new ReadOnlySolana("https://x/", (async (_u: string, init: any) => new Response(JSON.stringify({ jsonrpc: "2.0", id: JSON.parse(init.body).id, result: MAINNET_GENESIS }))) as any);
  await ok.ensureMainnet();
  const bad = new ReadOnlySolana("https://example.invalid/?api-key=SECRET", (async () => { throw new TypeError("fetch failed https://example.invalid/?api-key=SECRET"); }) as any);
  await assert.rejects(bad.call("getSlot"), (e: Error) => !e.message.includes("SECRET"), "the URL never appears in an error");
});

test("mainnet URL: env, file, or the Helius devnet key with the mainnet host", () => {
  const d = mkdtempSync(join(tmpdir(), "strat-"));
  mkdirSync(join(d, "keys/devnet"), { recursive: true });
  writeFileSync(join(d, "keys/devnet/solana-rpc-url"), "https://devnet.helius-rpc.com/?api-key=K\n");
  assert.equal(mainnetUrl({}, d), "https://mainnet.helius-rpc.com/?api-key=K");
  assert.equal(mainnetUrl({ STRAT_MAINNET_RPC: "https://m/" }, d), "https://m/");
  writeFileSync(join(d, "keys/devnet/solana-rpc-url"), "https://api.devnet.solana.com\n");
  assert.throws(() => mainnetUrl({}, d), /no mainnet read RPC/);
});

// ------------------------------------------------------------ PumpSwap math

function feeCfgBuf(): Buffer {
  const parts: Buffer[] = [Buffer.alloc(8), Buffer.from([255]), Buffer.alloc(32)];
  const u64 = (n: number) => { const b = Buffer.alloc(8); b.writeBigUInt64LE(BigInt(n)); return b; };
  parts.push(u64(25), u64(5), u64(0));
  const tiers = [[0, 2, 93, 30], [420e9, 20, 5, 95], [98240e9, 20, 5, 5]];
  const n = Buffer.alloc(4); n.writeUInt32LE(tiers.length); parts.push(n);
  for (const [t, lp, pr, cr] of tiers) { const th = Buffer.alloc(16); th.writeBigUInt64LE(BigInt(t)); parts.push(th, u64(lp), u64(pr), u64(cr)); }
  return Buffer.concat(parts);
}

test("PumpSwap: fee tiers by market cap, fees on buy and sell", () => {
  const cfg = decodeFeeConfig(feeCfgBuf());
  assert.equal(cfg.tiers.length, 3);
  assert.equal(totalBps(feesFor(cfg, 100n * 10n ** 9n, true)), 125);
  assert.equal(totalBps(feesFor(cfg, 500n * 10n ** 9n, true)), 120);
  assert.equal(totalBps(feesFor(cfg, 10n ** 23n, true)), 30);
  assert.equal(feesFor(cfg, 500n * 10n ** 9n, false).creator, 0, "non-canonical pools pay no creator fee");
  const r = { base: 500_000_000_000_000n, quote: 100_000_000_000n };
  const f = feesFor(cfg, 500n * 10n ** 9n, true);
  const out = buyOut(r, f, 100_000_000n);
  const back = sellOut({ base: r.base - out, quote: r.quote + 100_000_000n }, f, out);
  const lossBps = Number(((100_000_000n - back) * 10_000n) / 100_000_000n);
  // 120 bps taken from the buy input (120/1.012 effective) plus 120 bps off the sell output
  assert.ok(lossBps >= 236 && lossBps <= 240, `a round trip costs about two fees (${lossBps} bps)`);
});

// ------------------------------------------------------------ spec

test("spec: canonical hash ignores key order; bounds; conditions; size rule", () => {
  const a = spec();
  const rev = (v: any): any => Array.isArray(v) ? v.map(rev) : v && typeof v === "object" ? Object.fromEntries(Object.keys(v).reverse().map((k) => [k, rev(v[k])])) : v;
  const b = rev(a);
  assert.equal(specHash(a), specHash(b));
  assert.equal(canonicalJson({ b: 1, a: [{ d: 1, c: 2 }] }), '{"a":[{"c":2,"d":1}],"b":1}');
  assert.equal(conditionsHash(a.conditions), conditionsHash([...a.conditions].reverse()));
  assert.deepEqual(validate(a), []);
  assert.ok(validate(spec({ stop_bps: 50 })).length > 0);
  assert.equal(holds(a, { pool_sol: 70, ret_15m: 0.05 }).ok, true);
  assert.equal(holds(a, { pool_sol: 70, ret_15m: null }).ok, false, "a missing signal never passes");
  // equity 1 SOL: fraction 0.1 SOL, risk 0.01 / 8% = 0.125 SOL, pool 100 SOL x 20 bps = 0.2 SOL, max 0.1
  const s = sizeFor(a, 1_000_000_000n, 100_000_000_000n);
  assert.equal(s.size, 100_000_000n);
  assert.equal(s.maxRisk, 10_000_000n);
  assert.equal(sizeFor(a, 1_000_000_000n, 20_000_000_000n).bound, "pool share");
});

// ------------------------------------------------------------ simulated execution

test("sim: a fill worse than the limit is no fill; stop, target and time exits", () => {
  const f = { lp: 20, protocol: 5, creator: 95 };
  const r = { base: 500_000_000_000_000n, quote: 100_000_000_000n };
  const q = quote(r, f, 100_000_000n, 10);
  assert.ok(q.impact_bps > 100 && q.impact_bps < 140, `impact includes the fee (${q.impact_bps})`);
  assert.equal(fillBuy(q, r, f, 14, 150).ok, true);
  const moved = { base: r.base * 98n / 100n, quote: r.quote * 102n / 100n };
  const nf = fillBuy(q, moved, f, 14, 150);
  assert.equal(nf.ok, false);
  assert.ok(nf.slippage_bps > 150);
  assert.equal(exitDue(91_000_000n, 100_000_000n, 800, 1500, 0, 600), "stop");
  assert.equal(exitDue(116_000_000n, 100_000_000n, 800, 1500, 0, 600), "target");
  assert.equal(exitDue(100_000_000n, 100_000_000n, 800, 1500, 600_000, 600), "time");
  assert.equal(exitDue(100_000_000n, 100_000_000n, 800, 1500, 1000, 600), null);
});

test("signals: returns, volatility, flow, dev selling and concentration from the series", () => {
  const t0 = 1_000_000_000;
  const obs = Array.from({ length: 46 }, (_, i) => ({ slot: 100 + i, t: t0 + i * 20_000, base: (1_000_000_000_000n - BigInt(i) * 1_000_000_000n).toString(), quote: (100_000_000_000n + BigInt(i) * 100_000_000n).toString(), dev: (i < 40 ? 50_000_000_000n : 40_000_000_000n).toString() }));
  const w = { pool: { address: "P", baseVault: "BV", creator: "C" } as any, supply: 1_000_000_000_000n, decimals: 6, tokenProgram: "T", devAta: "D", obs, trades: 1 } as Watched;
  const v = signals(w, { lp: 20, protocol: 5, creator: 95 }, { slot: 1, holders: [{ address: "BV", owner: "P", amount: "800000000000", excluded: true }, { address: "h1", owner: "o1", amount: "100000000000", excluded: false }, { address: "h2", owner: "o2", amount: "50000000000", excluded: false }], lp: [], lpRead: true });
  assert.ok((v.ret_5m ?? 0) > 0 && (v.ret_15m ?? 0) > (v.ret_5m ?? 0));
  assert.equal(v.ret_60m, null, "15 minutes of series cannot answer a 60-minute window");
  assert.equal(v.dev_sold_15m, 1);
  assert.ok(Math.abs((v.top10_share ?? 0) - 0.15) < 1e-9, "the pool vault is excluded");
  assert.equal(v.lp_wallet_share, 0, "LP burned: nobody can pull it");
  assert.ok((v.flow_15m ?? 0) > 0);
});

// ------------------------------------------------------------ the analyzer

const snap = (avail: Record<string, number | null>, insp: string[] = ["pool_sol", "ret_15m", "fee_bps"]): Snapshot => ({ slot: 1000, available: avail as any, inspected: insp as any, conditions_hash: "c", tick_ms: 20_000 });
const rec = (o: Partial<TradeRecord> & { proceeds?: string; reason?: "stop" | "target" | "time" } = {}): TradeRecord => ({
  size: "100000000", max_risk: "10000000", quote: { out: "1000", impact_bps: 120, slot: 1000 }, fill: { ok: true, slot: 1004, out: "990", slippage_bps: 10, reason: "filled" },
  exit: { reason: o.reason ?? "stop", trigger_slot: 1100, trigger_value: "91000000", slot: 1104, proceeds: o.proceeds ?? "91500000", slippage_bps: 20 },
  timing: { decision_ms: 0, commit_seen_ms: 1500, fill_ms: 2000, latency_slots_model: 4 }, ...o,
});
const calm = { pool_sol: 100, ret_15m: 0.04, fee_bps: 120, top10_share: 0.2, dev_share: 0.01, vol_15m: 0.002 };

test("analyzer: VARIANCE only when every bucket passes and losses fit the stated rate", () => {
  const r = analyze(spec(), snap(calm), rec(), { outcomes: [], version_trades: 0, version_losses: 0 });
  assert.equal(r.outcome, "loss");
  assert.equal(r.buckets, BIT.VARIANCE);
  assert.deepEqual(r.names, ["VARIANCE"]);
  // 9 losses in 10 against a stated 55% loss rate: the edge is wrong, THESIS
  const t = analyze(spec(), snap(calm), rec(), { outcomes: [], version_trades: 9, version_losses: 8 });
  assert.equal(t.buckets, BIT.THESIS);
});

test("analyzer: INFORMATION names red flags that were available and not inspected", () => {
  const r = analyze(spec(), snap({ ...calm, top10_share: 0.55, dev_sold_15m: 1 }), rec(), { outcomes: [], version_trades: 0, version_losses: 0 });
  assert.ok(r.buckets & BIT.INFORMATION);
  assert.ok(!(r.buckets & BIT.VARIANCE));
  const ev = r.verdicts.find((v) => v.bucket === "INFORMATION")!.evidence.join("\n");
  assert.match(ev, /top10_share = 0.55 was available at slot 1000 and not inspected/);
  assert.match(ev, /dev_sold_15m = 1/);
  // inspected but let through
  const r2 = analyze(spec({ conditions: [{ signal: "top10_share", op: "<=", value: 0.6 }] }), snap({ ...calm, top10_share: 0.55 }, ["top10_share", "pool_sol", "fee_bps"]), rec(), { outcomes: [], version_trades: 0, version_losses: 0 });
  assert.match(r2.verdicts[0].evidence[0], /inspected, but the condition let it through/);
});

test("analyzer: RISK_MANAGEMENT for a stop inside the noise and for a gap; SIZING over max risk", () => {
  const noisy = analyze(spec(), snap({ ...calm, vol_15m: 0.03 }), rec(), { outcomes: [], version_trades: 0, version_losses: 0 });
  assert.ok(noisy.buckets & BIT.RISK_MANAGEMENT, "8% stop vs 0.03 x sqrt(60) = 23% sigma");
  const gap = analyze(spec(), snap(calm), rec({ proceeds: "80000000" }), { outcomes: [], version_trades: 0, version_losses: 0 });
  assert.ok(gap.buckets & BIT.RISK_MANAGEMENT);
  assert.ok(gap.buckets & BIT.SIZING, "lost 0.02 SOL against a 0.01 SOL max risk");
  assert.match(gap.verdicts.find((v) => v.bucket === "RISK_MANAGEMENT")!.evidence.join(" "), /gapped through the stop/);
  assert.equal(gap.verdicts.find((v) => v.bucket === "LEVERAGE")!.status, "not_applicable");
});

test("analyzer: EXECUTION on no fill; THESIS base rate; a lucky win is a finding", () => {
  const nf = analyze(spec(), snap(calm), rec({ fill: { ok: false, slot: 1004, out: "0", slippage_bps: 300, reason: "fill 300 bps worse" }, exit: null }), { outcomes: [], version_trades: 0, version_losses: 0 });
  assert.equal(nf.outcome, "no_fill");
  assert.equal(nf.buckets, BIT.EXECUTION);
  assert.equal(nf.pnl_lamports, "0");
  const th = analyze(spec(), snap(calm), rec(), { outcomes: ["loss", "loss", "loss", "loss", "loss", "loss"], version_trades: 0, version_losses: 0 });
  assert.ok(th.buckets & BIT.THESIS, "0 wins in 7 firings at a stated 45%");
  assert.ok(binomCdf(0, 7, 0.45) < 0.05);
  const lucky = analyze(spec(), snap({ ...calm, dev_share: 0.2 }), rec({ reason: "target", proceeds: "116000000" }), { outcomes: [], version_trades: 0, version_losses: 0 });
  assert.equal(lucky.outcome, "win");
  assert.ok(lucky.buckets & BIT.INFORMATION);
  assert.match(lucky.findings[0], /lucky win/);
});

// ------------------------------------------------------------ records and knowledge

function vrec(id: number, parent: number, claimed: number, height: number, superseded = 0): VersionRec {
  return { id, author: "A", line: 1, parent, hash: "00".repeat(32), spec_uri: "", thought_ref: "", claimed, motives: parent ? [1] : [], height, superseded_by: superseded, open_entry: 0, entries: 0, stats: { trades: 0, wins: 0, losses: 0, flats: 0, no_fills: 0, pnl_sum: "0", lucky_wins: 0, buckets: {} } };
}
function trec(id: number, version: number, height: number, buckets: number, pnl: number): TradeRec {
  return { id, author: "A", version, snapshot: "", size: "100", price_e12: "1", quote_out: "1", stop_bps: 800, target_bps: 1500, max_risk: "10", decision_slot: 1, decision_ref: "r", height,
    exit: { fill_slot: 2, fill_out: "1", fill_price_e12: "1", exit_slot: 3, reason: "stop", proceeds: String(100 + pnl), pnl: String(pnl), record: "", height: height + 1 },
    postmortem: { result: "", outcome: pnl > 0 ? "win" : "loss", buckets, names: [], analyzer: "", thought_ref: "", height: height + 2, used_by: 0 } };
}

test("knowledge: forward only, minimum sample, learned only on a lower fault rate", () => {
  const R = BIT.RISK_MANAGEMENT;
  const v1 = vrec(1, 0, 0, 10, 2), v2 = vrec(2, 1, 5, 500);
  const vs = new Map([[1, v1], [2, v2]]);
  const before = Array.from({ length: 12 }, (_, i) => trec(i + 1, 1, 20 + i * 10, i % 2 ? R : 0, -5));
  let k = knowledgeOf(v2, vs, before)!;
  assert.equal(k.status, "not_enough_trades");
  assert.match(k.status_text, /not enough trades yet: 0 of 10/);
  const after = Array.from({ length: MIN_FORWARD }, (_, i) => trec(100 + i, 2, 600 + i * 10, i === 0 ? R : 0, i % 3 ? 4 : -5));
  k = knowledgeOf(v2, vs, [...before, ...after])!;
  assert.equal(k.status, "learned");
  assert.equal(k.window, 10);
  assert.equal(k.baseline.faults, 5, "the parent's LAST 10 trades");
  assert.equal(k.forward.faults, 1);
  assert.ok(k.fisher_p_one_sided! > 0 && k.fisher_p_one_sided! < 0.1);
  const worse = after.map((t) => ({ ...t, postmortem: { ...t.postmortem!, buckets: R } }));
  assert.equal(knowledgeOf(v2, vs, [...before, ...worse])!.status, "not_learned");
  assert.equal(knowledgeOf(v2, vs, [...before.slice(0, 4), ...after])!.status, "baseline_too_small");
  // a parent trade committed after the child (impossible on chain, but never counted) and the child's
  // trades are never re-scored: each keeps its own committed buckets
  assert.equal(knowledgeOf(v2, vs, [...before, trec(99, 1, 700, R, -5), ...after])!.baseline.faults, 5);
  assert.equal(records([v1, v2], [...before, ...after])[1].knowledge!.status, "learned");
  assert.ok(Math.abs(fisherLower(0, 10, 10, 10) - 1 / 184756) < 1e-9);
});

test("records: the contract's version and trade layouts decode", () => {
  const W = (parts: Buffer[]) => Buffer.concat(parts);
  const u64 = (n: number | bigint) => { const b = Buffer.alloc(8); b.writeBigInt64LE(BigInt(n)); return b; };
  const str = (s: string) => { const b = Buffer.alloc(4); b.writeUInt32LE(Buffer.byteLength(s)); return W([b, Buffer.from(s)]); };
  const ver = W([Buffer.alloc(32, 1), u64(1), u64(0), Buffer.alloc(32, 2), str("blob:x"), str("req"), Buffer.from([0, 1]), u64(9), u64(77), u64(0), u64(0), u64(3),
    u64(3), u64(1), u64(2), u64(0), u64(0), u64(-1234), u64(0), ...[1, 0, 0, 0, 2, 0, 0].map(u64)]);
  const v = decodeVersion(1, ver);
  assert.equal(v.motives[0], 9);
  assert.equal(v.height, 77);
  assert.equal(v.stats.pnl_sum, "-1234");
  assert.equal(v.stats.buckets.RISK_MANAGEMENT, 2);
  const e = call.enter({ version: 1, snapshot: "ab".repeat(32), size: 5n, priceE12: 6n, quoteOut: 7n, stopBps: 800, targetBps: 1500, maxRisk: 8n, decisionSlot: 9, decisionRef: "req" });
  assert.equal(e[0], 5);
  assert.equal(Buffer.from(e.subarray(1, 6)).toString(), "enter");
  const tr = W([Buffer.alloc(32, 1), u64(1), Buffer.alloc(32, 3), u64(5), u64(6), u64(7), Buffer.from([32, 3, 0, 0, 0xdc, 5, 0, 0]), u64(8), u64(9), str("req"), u64(50), Buffer.from([0]), Buffer.from([0])]);
  const t = decodeTrade(1, tr);
  assert.equal(t.stop_bps, 800);
  assert.equal(t.target_bps, 1500);
  assert.equal(t.exit, null);
});

// ------------------------------------------------------------ the paid thoughts' parsers

test("thought parsers: bounded menus, veto only on a clear SKIP, fixes stay inside the bounds", () => {
  const a = parseAuthor("STOP 9 TARGET 30 HOLD 25 WIN 90 because deep pools trend");
  assert.deepEqual([a.stop_bps, a.target_bps, a.max_hold_s, a.win_rate], [800, 2500, 20 * 60, 0.7]);
  assert.equal(a.rationale, "deep pools trend");
  assert.deepEqual(parseAuthor("whatever"), { stop_bps: 800, target_bps: 1500, max_hold_s: 1200, win_rate: 0.4, rationale: "" });
  assert.equal(parseDecision("SKIP, too hot"), "skip");
  assert.equal(parseDecision("TAKE. Not a skip"), "take");
  assert.equal(parseDecision("hmm"), "take");
  assert.equal(parseFix("I pick B because", 3), 1);
  assert.equal(parseFix("C", 2), 0);
  assert.equal(parseLesson("LESSON: widen the stop https://evil.example"), "");
  assert.equal(parseLesson("LESSON: the stop sat inside the noise.\nmore"), "the stop sat inside the noise.");
  const s = spec();
  for (const b of ["INFORMATION", "THESIS", "EXECUTION", "SIZING", "RISK_MANAGEMENT"] as const) {
    const fx = fixesFor(b, s, { missed: ["top10_share", "dev_share"], observedWinRate: 0.3, sigmaHoldBps: 1500, medianVol: 0.004 });
    assert.ok(fx.length >= 1, b);
    for (const f of fx) assert.deepEqual(validate(f.apply(s)), [], `${b}: ${f.text}`);
  }
  const info = fixesFor("INFORMATION", s, { missed: ["top10_share"], observedWinRate: null, sigmaHoldBps: null, medianVol: null })[0].apply(s);
  assert.ok(info.conditions.some((c) => c.signal === "top10_share" && c.op === "<=" && c.value === 0.4));
});

// ------------------------------------------------------------ one agent end to end (fakes)

class FakeChain {
  height = 100;
  versions: any[] = [];
  entries: any[] = [];
  txs = new Map<string, any>();
  n = 0;
  rpc = { call: async (m: string, p: any) => (m === "getTxStatus" ? { status: "preconfirmed" } : m === "putBlob" ? { blob_id: null } : null) };
  async height_() { return this.height; }
  async sendContract(_w: any, b: any) {
    const a = Buffer.from(b.args);
    const name = a.subarray(1, 1 + a[0]).toString();
    let ret = Buffer.alloc(8);
    this.height++;
    if (name === "publish") { this.versions.push({ height: this.height, claimed: a[1 + a[0] + 32 + 8] }); ret.writeBigUInt64LE(BigInt(this.versions.length)); }
    if (name === "enter") { this.entries.push({ height: this.height }); ret.writeBigUInt64LE(BigInt(this.entries.length)); }
    if (name === "postmortem") ret = Buffer.from([2]);
    const tx = `tx${++this.n}`;
    this.txs.set(tx, { name, height: this.height, ret });
    return { txHash: tx, nonce: 0n, fee: 1n, sentAt: Date.now(), bytes: a.length };
  }
  async waitIncluded(tx: string) { const t = this.txs.get(tx); return { height: t.height, receipt: { ok: true, status: "ok", return: t.ret.toString("hex") }, at: Date.now() }; }
}

test("agent: commit before fill, exit in a later block, post-mortem, then a revision citing it", async () => {
  const dir = mkdtempSync(join(tmpdir(), "strat-agent-"));
  const chain = new FakeChain();
  const cadre: any = { rpc: chain.rpc, sendContract: chain.sendContract.bind(chain), waitIncluded: chain.waitIncluded.bind(chain), height: async () => chain.height };
  let px = 100_000_000_000n;
  const order: string[] = [];
  const t0 = Date.now() - 30 * 60_000;
  const obs = Array.from({ length: 90 }, (_, i) => ({ slot: 1000 + i, t: t0 + i * 20_000, base: "1000000000000000", quote: (100_000_000_000n + BigInt(i) * 100_000_000n).toString(), dev: "0" }));
  const w: Watched = { pool: { address: "POOL", baseMint: "MINT", baseVault: "BV", quoteVault: "QV", lpMint: "LP", coinCreator: "CC", creator: "CR" } as any, supply: 1_000_000_000_000_000n, decimals: 6, tokenProgram: "T", devAta: "D", obs, trades: 3 };
  let slot = 2000;
  const market: any = {
    pools: new Map([["POOL", w]]),
    fees: () => ({ lp: 20, protocol: 5, creator: 95 }),
    deep: async () => ({ slot, holders: [{ address: "h", owner: "o", amount: String(10n ** 17n), excluded: false }], lp: [], lpRead: true }),
    readPool: async (_a: string, minSlot = 0) => { slot = Math.max(slot + 1, minSlot); order.push(`read:${slot}`); return { slot, base: 1_000_000_000_000_000n, quote: px, t: Date.now() }; },
  };
  const thoughts: string[] = [];
  const think = async (m: any[], _n: number) => { const sys = m[0].content as string; thoughts.push(sys.slice(0, 20)); return { text: sys.includes("one letter") ? "A" : sys.includes("TAKE or SKIP") ? "TAKE" : sys.includes("LESSON") ? "LESSON: the stop was too tight." : "STOP 5 TARGET 25 HOLD 10 WIN 60 because trend", requestId: `req_${thoughts.length}` }; };
  const store = new BlobStore({ dir: join(dir, "blobs") });
  const wallet = walletFromSeed(new Uint8Array(32).fill(7));
  const agent = new PracticeAgent({ handle: "t", name: "T", persona: "T", base: { conditions: [{ signal: "pool_sol", op: ">=", value: 60 }, { signal: "ret_15m", op: ">=", value: 0.01 }], size: { equity_fraction_bps: 1000, max_lamports: 100_000_000, max_pool_share_bps: 20 }, max_risk_bps: 100, max_slippage_bps: 150, practice_balance_lamports: 1_000_000_000 } },
    { cadre, contract: "C", market, store, think, wallet, stateFile: join(dir, "t.json"), tickMs: 20_000, analyzerHash: "aa".repeat(32), log: (e, x) => order.push(`${e}${x.what ? ":" + x.what : ""}${x.message ? ":" + x.message : ""}${x.failed ? ":" + JSON.stringify(x.failed) : ""}`), reviseAfter: 1 });
  px = 110_000_000_000n; // the decision read: above the price 15 minutes back (about 104.5 SOL)
  await agent.tick();
  assert.equal(chain.versions.length, 1);
  assert.ok(agent.s.open, `entered: ${order.join(" ")}`);
  const iEnter = order.indexOf("committed:enter");
  const iFill = order.findIndex((x, i) => i > iEnter && x.startsWith("read:"));
  assert.ok(iEnter >= 0 && iFill > iEnter, "the fill is read only after the entry commitment");
  assert.ok(agent.s.open!.fill.slot >= agent.s.open!.quote.slot + 4, "the fill is at least the modelled 4 slots after the quote");
  // the price falls through the stop on the next observation
  w.obs.push({ slot: slot + 10, t: Date.now(), base: "1000000000000000", quote: "90000000000", dev: "0" });
  px = 89_000_000_000n;
  await agent.tick();
  assert.equal(agent.s.open?.stage, "exiting", "no exit in the entry's own block");
  chain.height++; // the next Cadre block
  await agent.tick();
  assert.equal(agent.s.open, null, `exited and explained: ${order.join(" ")}`);
  assert.equal(agent.s.history.length, 1);
  const h = agent.s.history[0];
  assert.equal(h.outcome, "loss");
  assert.ok(chain.entries[0].height < chain.txs.get([...chain.txs.keys()].find((k) => chain.txs.get(k).name === "exit")!).height, "exit lands after the entry");
  if (h.buckets & 31) {
    assert.equal(chain.versions.length, 2, "a fixable bucket with reviseAfter 1 publishes a revision");
    assert.ok(agent.s.history[0].used);
    assert.equal(agent.s.spec!.parent, specHash({ ...agent.s.spec!, parent: null } as any) === agent.s.spec_id ? null : agent.s.spec!.parent);
  } else assert.equal(chain.versions.length, 1, "VARIANCE alone never revises");
  assert.ok(thoughts.length >= 3, "author, decision and lesson are paid thoughts");
});
