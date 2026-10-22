// The pre-confirmation gate wired into the trading runtime (pearl-sol docs/SPEED.md 2.5): Cadre.waitSettled
// on a scripted node, a Cadre AMM fill settled by its CADRE notional before the trader moves on, and a
// receipt Post counted from preconfirmed (it moves no value) with the stated risk in the log. No network.
import { test } from "node:test";
import assert from "node:assert/strict";
import { mkdtempSync } from "node:fs";
import { join } from "node:path";
import { tmpdir } from "node:os";
import { Cadre } from "../src/cadre.ts";
import { Trader, type RiskRules } from "../src/trader.ts";
import type { Book, Fill, MarketEvent, Venue } from "../src/venues.ts";

const U = 1_000_000_000n;
const H = "cd".repeat(32);
const preconf = (confidence: string) => ({ status: "preconfirmed", preconf: { weak_count: 3, weak_miners: 2, weak_share: 1, first_seen_ms: 1, latest_age_ms: 50, confidence } });
const included = { status: "included", block_hash: "ee".repeat(32), height: 6010, confirmations: 1, final: false, finality_k: 60 };

/** A node answering getTxStatus from a script (the last answer repeats). */
function node(steps: any[]) {
  let i = 0;
  return { calls: [] as string[], async call(m: string) { this.calls.push(m); return steps[Math.min(i++, steps.length - 1)]; } };
}

test("Cadre.waitSettled: a zero-notional tx acts at preconfirmed medium; 20 CADRE waits for included", async () => {
  const c = new Cadre("http://unused");
  (c as any).rpc = node([{ status: "pending" }, preconf("low"), preconf("medium")]);
  const a = await c.waitSettled(H, 0n, undefined, { pollMs: 1 });
  assert.equal(a.status, "preconfirmed");
  assert.match(a.risk, /not binding/);
  (c as any).rpc = node([preconf("high"), preconf("high"), included]);
  const b = await c.waitSettled(H, 20n * U, undefined, { pollMs: 1 });
  assert.equal(b.need, "included");
  assert.equal(b.status, "included");
  assert.match(b.risk, /1 of 60 confirmations/);
  (c as any).rpc = node([preconf("high")]);
  await assert.rejects(c.waitSettled(H, 2000n * U, undefined, { pollMs: 1, timeoutMs: 20 }), /needs final/);
});

class FakeVenue implements Venue {
  unit = "CADRE"; baseSymbol = "T"; baseDecimals = 9;
  trades: { side: 0 | 1; amountIn: bigint }[] = [];
  r = { base: 1_000_000_000_000n, quote: 100_000_000_000_000n };
  b: Book = { quote: 1_000_000_000_000n, base: 0n, realized: 0n };
  name: string;
  constructor(name: string) { this.name = name; }
  async start() {}
  stop() {}
  async reserves() { return this.r; }
  async book() { return this.b; }
  async trade(side: 0 | 1, amountIn: bigint, minOut: bigint): Promise<Fill> {
    this.trades.push({ side, amountIn });
    return { ok: true, venue: this.name, side, amountIn, minOut, out: minOut, ref: H, t: { built: 0, sent: 1, landed: 2 }, landedAt: "height:6010", fee: 1000n };
  }
  receiptRef(f: Fill) { return `cadre:${f.ref}`; }
  describe() { return { venue: this.name, pool: "P" }; }
}

const risk: RiskRules = { maxOrder: 5n * U, maxPosition: 50n * U, dailyLossStop: 5n * U, cooldownMs: 0, maxOrdersPerHour: 100, slippageBps: 200, maxChaseBps: 300 };

function mk(venue: string, postReceipts: boolean) {
  const v = new FakeVenue(venue);
  const dir = mkdtempSync(join(tmpdir(), "preconf-hook-"));
  const logs: [string, any][] = [];
  const settled: [string, bigint, any][] = [];
  const cadre = {
    async waitSettled(tx: string, notional: bigint, _p?: unknown, o?: unknown) {
      settled.push([tx, notional, o]);
      return notional === 0n
        ? { tx_hash: tx, need: "preconfirmed", status: "preconfirmed", tx_status: { height: null }, risk: "acted on preconfirmed (3 weak blocks, 2 miners, medium): not binding", waited_ms: 900 }
        : { tx_hash: tx, need: "included", status: "included", tx_status: { height: 6010 }, risk: "acted on included (1 of 60 confirmations): a reorg can still undo it until final", waited_ms: 0 };
    },
  };
  const t = new Trader({
    handle: "h", persona: "p", house: false, venue: v, brain: null, cadre: cadre as any, poster: null as any, risk,
    thinkEveryMs: 1e9, killFile: join(dir, "STOP"), latencyLog: join(dir, "lat.jsonl"), houseBook: new Map(), log: (k: string, d: any) => logs.push([k, d]), postReceipts,
    sendPost: async () => ({ txHash: "ab".repeat(32), nonce: 1n }),
  } as any);
  t.book = v.b;
  t.strategy = { stance: "buy", conviction: 5, setAt: 0, requestId: "req_1", text: "" };
  t.refPrice = 100;
  return { t, v, logs, settled };
}
const ev = (v: FakeVenue): MarketEvent => ({ venue: v.name, base: v.r.base, quote: v.r.quote, at: performance.now(), ref: "height:6009" });

test("a cadre-amm fill is settled by its CADRE notional (buy: amount in) and the stage + risk are logged", async () => {
  const { t, v, logs, settled } = mk("cadre-amm", false);
  await t.onEvent(ev(v));
  assert.equal(v.trades.length, 1);
  assert.deepEqual(settled.map((s) => [s[0], s[1]]), [[H, v.trades[0].amountIn]]);
  const s = logs.find(([k]) => k === "settled")!;
  assert.equal(s[1].status, "included");
  assert.match(s[1].risk, /reorg can still undo/);
});

test("a Solana venue fill does not wait on Cadre; its receipt Post counts from preconfirmed with the stated risk", async () => {
  const { t, v, logs, settled } = mk("pumpswap", true);
  await t.onEvent(ev(v));
  assert.equal(v.trades.length, 1);
  assert.equal(settled.length, 1, "only the receipt Post goes through the gate");
  assert.equal(settled[0][0], "ab".repeat(32));
  assert.equal(settled[0][1], 0n);
  const r = logs.find(([k]) => k === "receipt")!;
  assert.equal(r[1].status, "preconfirmed");
  assert.match(r[1].risk, /not binding/);
  assert.ok(!logs.some(([k]) => k === "receipt_failed"));
  assert.ok(!JSON.stringify(logs).includes(String.fromCharCode(0x2014)));
});
