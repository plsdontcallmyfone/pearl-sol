// Prepaid inference for hosted agents (src/inference.ts + AgentRunner.topup/think/practiceWaitMs):
// tickets sized for value against the live quote, refused top-ups and used-up rows hold quietly
// (one feed line) until the next UTC day or a wallet change, and practice thoughts are paced to
// what the day's rows and SOL cap sustain.
import assert from "node:assert/strict";
import { createHash, randomBytes } from "node:crypto";
import { mkdtempSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { test } from "node:test";
import { b58encode } from "../../src/b58.ts";
import { walletFromSeed } from "../../src/wallet.ts";
import { AgentRunner } from "../src/agent.ts";
import { MAX_TICKET_ROWS, need402, nextUtcDay, practiceWaitMs, ticketLadder } from "../src/inference.ts";
import { PACED, practiceLog } from "../src/practice.ts";
import { defaultSpecV2, specSha256 } from "../src/spec.ts";

const seedOf = (s: string) => createHash("sha256").update(s).digest();
const DAY = 86_400_000;
const T0 = Date.UTC(2026, 9, 23, 2, 15); // 02:15 UTC, like the box 0 journal

// A price fixture fitted to the box 0 quotes of 2026-10-23 (1,024 rows 25,135,765; 4,096 rows
// 25,209,724; 8,192 rows 25,308,335 lamports): a fixed order cost plus about 24 lamports a row.
const price = (rows: number) => 25_110_797n + BigInt(Math.round(24.08 * rows));

test("box 0 quotes: the fixture matches the measured prices to within 0.01%", () => {
  for (const [rows, measured] of [[1024, 25_135_765n], [4096, 25_209_724n], [8192, 25_308_335n]] as const) {
    const d = price(rows) > measured ? price(rows) - measured : measured - price(rows);
    assert.ok(d * 10_000n < measured, `${rows}: ${price(rows)} vs ${measured}`);
  }
});

test("ticketLadder: largest first (gateway max, default, least that fits), multiples of 4, never above the max", () => {
  assert.deepEqual(ticketLadder(345, { defaultRows: 4096, maxRows: 8192, minFree: 256 }), [8192, 4096, 604]);
  assert.deepEqual(ticketLadder(0, { defaultRows: 4096, maxRows: 8192, minFree: 256 }), [8192, 4096, 256]);
  assert.deepEqual(ticketLadder(5000, { defaultRows: 4096, maxRows: 8192, minFree: 256 }), [8192, 5256]);
  assert.deepEqual(ticketLadder(100, { defaultRows: 4096, maxRows: 4096, minFree: 256 }), [4096, 356]);
  assert.deepEqual(ticketLadder(8000, { defaultRows: 4096, maxRows: 8192, minFree: 256 }), [], "a thought larger than the largest ticket");
  assert.equal(MAX_TICKET_ROWS, 8192);
});

test("need402, nextUtcDay", () => {
  assert.equal(need402(new Error('gateway 402: {"error":{"code":402,"message":"no prepaid balance with 345 free rows for Aig6: POST /v1/balance/topup"}}')), 345);
  assert.equal(need402(new Error("gateway 500: boom")), null);
  assert.equal(nextUtcDay(T0), Date.UTC(2026, 9, 24));
  assert.equal(nextUtcDay(Date.UTC(2026, 9, 24)), Date.UTC(2026, 9, 25));
});

test("practiceWaitMs: rows left spread over the rest of the UTC day; none left = wait for 00:00", () => {
  const now = Date.UTC(2026, 9, 23, 0, 0);
  const base = { now, lastAt: now, avgRows: 300, rowsLeft: 12_288, rowsFree: 8192, capLeft: 80_000_000n, quote: { rows: 8192, price: price(8192).toString() } };
  assert.equal(practiceWaitMs({ ...base, lastAt: 0 }), 0, "the first practice thought runs at once");
  // 24 h x 300 / 12,288 rows = 35.2 min between thoughts: about 41 a day
  assert.equal(practiceWaitMs(base), Math.floor((DAY * 300) / 12_288));
  // the SOL cap binds when it buys fewer rows than the row budget: 8,192 free + 0 tickets
  assert.equal(practiceWaitMs({ ...base, capLeft: 20_000_000n }), Math.floor((DAY * 300) / 8192));
  // nothing left today: wait until 00:00 UTC
  assert.equal(practiceWaitMs({ ...base, rowsLeft: 200 }), DAY);
  assert.equal(practiceWaitMs({ ...base, rowsFree: 0, capLeft: 20_000_000n }), DAY);
});

test("practiceLog drops paced waits (no runtime line, no feed line)", () => {
  const logs: string[] = [], notes: string[] = [];
  const f = practiceLog({ log: (e) => logs.push(e), note: (k) => notes.push(k) });
  f("error", { message: `${PACED} next practice thought in 60 s` });
  assert.deepEqual([logs, notes], [[], []]);
  f("error", { message: "gateway 500" });
  assert.deepEqual([logs, notes], [["practice_error"], ["practice_error"]]);
});

// ---------------------------------------------------------------- the runner, on stubs

interface Stub { runner: AgentRunner; clock: { t: number }; quotes: number[]; paid: number[]; chats: number; wallet: { lamports: bigint }; free: { rows: number }; events: any[] }

function runner(budget: { sol: string; rows: number }, o: { lamports?: bigint } = {}): Stub {
  const op = walletFromSeed(seedOf("tu-operator"));
  const w = walletFromSeed(seedOf(`tu-agent-${Math.random()}`));
  const spec = defaultSpecV2(w.address, op.address, {
    handle: "tu_agent", name: "TU", persona: "Posts plainly.", created_ms: T0,
    budget: { sol_lamports_per_day: budget.sol, cadre_units_per_day: "1000000000", rows_per_day: budget.rows },
  });
  const clock = { t: T0 };
  const st: Stub = { runner: null as any, clock, quotes: [], paid: [], chats: 0, wallet: { lamports: o.lamports ?? 1_000_000_000n }, free: { rows: 0 }, events: [] };
  const payTo = b58encode(randomBytes(32));
  const gateway = {
    topup: async (body: any, x?: string) => {
      const req = { scheme: "exact", network: "solana-devnet", maxAmountRequired: price(body.rows).toString(), payTo, extra: { assetKind: "native" } };
      if (!x) { st.quotes.push(body.rows); return { status: 402, body: { accepts: [req] } }; }
      st.paid.push(body.rows);
      st.free.rows += body.rows;
      st.wallet.lamports -= price(body.rows);
      return { status: 201, body: { ticket_id: "t", order: "1", rows: body.rows } };
    },
    balance: async () => ({ status: 200, body: { rows_free: st.free.rows } }),
    chat: async (_m: any, messages: any[]) => {
      st.chats++;
      const rows = 300;
      // the gateway needs one ticket with the request's rows free (here: all free rows in one)
      if (st.free.rows < rows) return { status: 402, body: { error: { code: 402, message: `no prepaid balance with ${rows} free rows for ${w.address}: POST /v1/balance/topup` } }, headers: new Headers() };
      st.free.rows -= rows;
      return { status: 200, body: { choices: [{ message: { content: `ok ${messages.length}` } }], cadre: { request_id: `req_${st.chats}`, rows } }, headers: new Headers() };
    },
  };
  const deps: any = {
    node: { call: async (m: string) => (m === "getParams" ? { max_balance_rows: 8192 } : null) },
    gateway, api: null, latestBlockhash: async () => b58encode(randomBytes(32)), now: () => clock.t, rand: () => 0.5, log: (e: any) => st.events.push(e),
  };
  const api: any = { must: async () => ({ solana: { lamports: st.wallet.lamports.toString(), spl: "0" } }) };
  const dir = mkdtempSync(join(tmpdir(), "agents-topup-"));
  const doc = { spec, sha256: specSha256(spec), sig: "" };
  st.runner = new AgentRunner(doc as any, w, deps, api, {
    stateFile: join(dir, "agent.json"), killFile: join(dir, "STOP"), allowHosts: [], solRpc: "http://sol.invalid",
    topupRows: 4096, maxTopupRows: 8192, minFreeRows: 256, cardUrl: () => "https://x.invalid",
  });
  return st;
}

const msgs = [{ role: "user", content: "say something short" }];
const feed = (s: Stub, kind: string) => s.runner.log2.filter((l) => l.kind === kind);

test("a 402 buys the largest ticket the cap allows (8,192 rows), not N + 256", async () => {
  const s = runner({ sol: "80000000", rows: 12_288 });
  const t = await s.runner.think(s.runner.me, msgs, 24, "practice");
  assert.ok(t, "thought served after one top-up");
  assert.deepEqual(s.quotes, [8192], "one quote: the largest fits");
  assert.deepEqual(s.paid, [8192]);
  assert.equal(s.runner.budget.st.topupLamports, price(8192).toString(), "booked at the quoted price");
  // the next 26 thoughts ride on that ticket: no further top-up
  for (let i = 0; i < 26; i++) assert.ok(await s.runner.think(s.runner.me, msgs, 24, "practice"));
  assert.deepEqual(s.paid, [8192]);
});

test("daily SOL cap: one hold until 00:00 UTC, one feed line, no quotes or chats retried; next day it buys 8,192 rows", async () => {
  // lead_336a0 on box 0: cap 30,000,000 with 9,013,040 already spent today
  const s = runner({ sol: "30000000", rows: 4096 });
  s.runner.budget.bookTopup(9_013_040n);
  assert.equal(await s.runner.think(s.runner.me, msgs, 24, "post"), null);
  assert.deepEqual(s.quotes, [8192, 4096, 556], "every size quoted once, all above the 20,986,960 left");
  assert.equal(s.runner.inf.hold?.reason, "sol_cap");
  assert.equal(s.runner.inf.hold?.until, nextUtcDay(T0));
  const chats = s.chats;
  for (let i = 0; i < 50; i++) {
    s.clock.t += 5_000;
    assert.equal(await s.runner.topup(300), false);
    assert.ok((await s.runner.ensureBalance()) === false);
  }
  assert.equal(s.quotes.length, 3, "held: nothing quoted again");
  assert.equal(s.chats, chats);
  assert.equal(feed(s, "inference_hold").length, 1, "logged once");
  assert.match(feed(s, "inference_hold")[0].text, /until 00:00 UTC.*daily SOL cap/);
  assert.ok(s.runner.practiceWaitMs() > 0, "practice waits while held");
  // 00:00 UTC: the day rolls, the hold lifts, the largest ticket fits the fresh 30,000,000 cap
  s.clock.t = nextUtcDay(T0) + 1_000;
  const t = await s.runner.think(s.runner.me, msgs, 24, "post");
  assert.ok(t);
  assert.deepEqual(s.paid, [8192]);
  assert.equal(s.runner.inf.hold, null);
});

test("wallet short: hold until the wallet balance changes (re-read once a minute), then buy", async () => {
  const s = runner({ sol: "80000000", rows: 12_288 }, { lamports: 10_000_000n });
  assert.equal(await s.runner.topup(0), false);
  assert.equal(s.runner.inf.hold?.reason, "wallet");
  assert.match(feed(s, "inference_hold")[0].text, /until its wallet holds more SOL/);
  s.clock.t += 30_000;
  s.wallet.lamports = 200_000_000n;
  assert.equal(await s.runner.topup(0), false, "not re-read before a minute");
  s.clock.t += 31_000;
  assert.equal(await s.runner.topup(0), true, "balance changed: the hold lifts and the top-up runs");
  assert.deepEqual(s.paid, [8192]);
  assert.equal(feed(s, "inference_hold").length, 1);
});

test("rows budget used: null without a gateway call, one feed line per UTC day (was budget_refused every tick)", async () => {
  const s = runner({ sol: "80000000", rows: 1500 });
  s.free.rows = 8192;
  s.runner.budget.commit("tu_agent", 0, 1400);
  for (let i = 0; i < 20; i++) assert.equal(await s.runner.think(s.runner.me, msgs, 24, "practice"), null);
  assert.equal(s.chats, 0);
  assert.equal(feed(s, "inference_hold").length, 1);
  assert.equal(s.events.filter((e) => e.event === "budget_refused").length, 0);
  s.clock.t = nextUtcDay(T0) + 1;
  assert.ok(await s.runner.think(s.runner.me, msgs, 24, "practice"), "a new day, new rows");
});

test("practice cadence on the runner: first thought at once, then spaced by the day's rows", async () => {
  const s = runner({ sol: "80000000", rows: 12_288 });
  s.clock.t = Date.UTC(2026, 9, 23, 0, 0);
  s.runner.budget.roll(s.clock.t);
  assert.equal(s.runner.practiceWaitMs(), 0);
  await s.runner.think(s.runner.me, msgs, 24, "practice");
  s.runner.inf.practiceAt = s.clock.t;
  const wait = s.runner.practiceWaitMs();
  // 300 rows booked over 1 thought; 11,988 rows left; SOL side 7,892 free + 2 more tickets
  assert.equal(wait, Math.floor((DAY * 300) / 11_988));
  s.clock.t += wait;
  assert.equal(s.runner.practiceWaitMs(), 0);
});
