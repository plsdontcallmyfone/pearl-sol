// Strategies routes (docs/STRATEGIES.md, docs/API.md "Strategies") against a fake node that serves
// the strategies contract's query layouts byte for byte (contracts/strategies/src/lib.rs), plus the
// durable blob store. Ports 10681 (node) and 10682 (api), the strategies row's test block.
import { test, before, after } from "node:test";
import assert from "node:assert/strict";
import { createServer, type Server } from "node:http";
import { mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { configFromEnv, Deps } from "../src/deps.ts";
import { build } from "../src/app.ts";
import { canonicalJson } from "../src/routes/strategies.ts";
import { shared } from "../src/shared.ts";
import { blake3 } from "../../../apps/explorer/src/blake3.ts";
import { key } from "./fake.ts";
import { b58decode } from "../../../apps/explorer/src/codec.ts";

const PORTS = { node: 10681, api: 10682 };
const A = key("lark"), B = key("wren"), CONTRACT = key("strategies");
const idOf = (s: string) => Buffer.from(blake3(Buffer.from(s, "utf8"))).toString("hex");
const RM = 1 << 4; // RISK_MANAGEMENT

class Wb {
  p: Buffer[] = [];
  u8(v: number) { this.p.push(Buffer.from([v])); return this; }
  u32(v: number) { const b = Buffer.alloc(4); b.writeUInt32LE(v); this.p.push(b); return this; }
  u64(v: number | bigint) { const b = Buffer.alloc(8); b.writeBigUInt64LE(BigInt(v)); this.p.push(b); return this; }
  i64(v: number | bigint) { const b = Buffer.alloc(8); b.writeBigInt64LE(BigInt(v)); this.p.push(b); return this; }
  addr(a: string) { this.p.push(Buffer.from(b58decode(a))); return this; }
  h(x: string) { this.p.push(Buffer.from(x, "hex")); return this; }
  s(t: string) { const b = Buffer.from(t); this.u32(b.length); this.p.push(b); return this; }
  hex() { return Buffer.concat(this.p).toString("hex"); }
}

// the spec blob of version 1, kept on the node only (the API adopts it on first read)
const SPEC1 = canonicalJson({ format: "cadre-strategy-v1", name: "tape_lark v1", parent: null, stop_bps: 500 });
const SNAP1 = canonicalJson({ format: "cadre-practice-snapshot-v1", slot: 1, pool: { address: "P" } });

interface V { id: number; author: string; line: number; parent: number; hash: string; claimed: number; motives: number[]; height: number; superseded_by: number; buckets: number[]; stats: number[]; pnl: number }
interface T { id: number; author: string; version: number; snapshot: string; height: number; pnl: number; outcome: number; buckets: number; used_by: number }
const versions: V[] = [];
const trades: T[] = [];
// version 1: 10 explained trades, 6 with RISK_MANAGEMENT; version 2 claims RISK_MANAGEMENT, cites 9 and 10; 10 trades, 2 with it
versions.push({ id: 1, author: A, line: 1, parent: 0, hash: idOf(SPEC1), claimed: 0, motives: [], height: 100, superseded_by: 2, buckets: [0, 0, 0, 0, 6, 0, 0], stats: [10, 4, 6, 0, 0], pnl: -2_000_000 });
for (let i = 1; i <= 10; i++) trades.push({ id: i, author: A, version: 1, snapshot: i === 1 ? idOf(SNAP1) : "ab".repeat(32), height: 100 + i, pnl: i <= 4 ? 1_000_000 : -1_000_000, outcome: i <= 4 ? 1 : 2, buckets: i > 4 ? RM : 0, used_by: i >= 9 ? 2 : 0 });
versions.push({ id: 2, author: A, line: 1, parent: 1, hash: "cd".repeat(32), claimed: 5, motives: [9, 10], height: 111, superseded_by: 0, buckets: [0, 0, 0, 0, 2, 0, 0], stats: [10, 8, 2, 0, 0], pnl: 6_000_000 });
for (let i = 11; i <= 20; i++) trades.push({ id: i, author: A, version: 2, snapshot: "ef".repeat(32), height: 101 + i, pnl: i <= 18 ? 1_000_000 : -1_000_000, outcome: i <= 18 ? 1 : 2, buckets: i > 18 ? RM : 0, used_by: 0 });
versions.push({ id: 3, author: B, line: 3, parent: 0, hash: "aa".repeat(32), claimed: 0, motives: [], height: 120, superseded_by: 0, buckets: [0, 0, 0, 0, 0, 0, 0], stats: [0, 0, 0, 0, 0], pnl: 0 });
// an open entry on version 3 (no exit yet)
trades.push({ id: 21, author: B, version: 3, snapshot: "bb".repeat(32), height: 125, pnl: 0, outcome: 0, buckets: 0, used_by: 0 });

function encVersion(v: V) {
  const w = new Wb().addr(v.author).u64(v.line).u64(v.parent).h(v.hash).s(`blob:${v.hash}`).s("req_1").u8(v.claimed).u8(v.motives.length);
  for (const m of v.motives) w.u64(m);
  w.u64(v.height).u64(v.superseded_by).u64(v.id === 3 ? 21 : 0).u64(trades.filter((t) => t.version === v.id).length);
  for (const x of v.stats) w.u64(x);
  w.i64(v.pnl).u64(0);
  for (const b of v.buckets) w.u64(b);
  return w.hex();
}
function encTrade(t: T) {
  const w = new Wb().addr(t.author).u64(t.version).h(t.snapshot).u64(100_000_000).u64(5).u64(7).u32(500).u32(1500).u64(1_000_000).u64(900).s("req_d").u64(t.height);
  if (t.outcome === 0) return w.u8(0).u8(0).hex();
  w.u8(1).u64(904).u64(7).u64(5).u64(950).u8(t.pnl > 0 ? 2 : 1).u64(100_000_000 + t.pnl).i64(t.pnl).h("12".repeat(32)).u64(t.height + 1);
  w.u8(1).h("34".repeat(32)).u8(t.outcome).u8(t.buckets).h("56".repeat(32)).s("req_l").u64(t.height + 2).u64(t.used_by);
  return w.hex();
}

function node(m: string, p: any): any {
  switch (m) {
    case "getChainInfo": return { chain_id: 7, genesis_hash: "cd".repeat(32), height: 200, finalized_height: 140, finality_k: 60 };
    case "getParams": return { min_fee: "1000", finality_k: 60 };
    case "getBlock": return null;
    case "listJobs": return { total: 0, jobs: [] };
    case "getBridgeEvents": return { withdrawals: [] };
    case "getBlob": return p.blob_id === idOf(SPEC1) ? { data: Buffer.from(SPEC1).toString("base64") } : null;
    case "queryContract": {
      if (p.address !== CONTRACT) return { ok: false, error: Buffer.from("no contract").toString("hex") };
      const a = Buffer.from(p.args, "hex");
      const name = a.subarray(1, 1 + a[0]).toString();
      const id = a.length >= 1 + a[0] + 8 ? Number(a.readBigUInt64LE(1 + a[0])) : 0;
      if (name === "counts") return { ok: true, return: new Wb().u64(versions.length).u64(trades.length).u64(50).hex() };
      if (name === "version") { const v = versions.find((x) => x.id === id); return v ? { ok: true, return: encVersion(v) } : { ok: false, error: "" }; }
      if (name === "trade") { const t = trades.find((x) => x.id === id); return t ? { ok: true, return: encTrade(t) } : { ok: false, error: "" }; }
      return { ok: false, error: "" };
    }
    default: throw new Error(`unknown method ${m}`);
  }
}

let nodeSrv: Server, apiSrv: Server, dir = "", base = "";
const listen = (s: Server, port: number) => new Promise<void>((r, j) => s.once("error", j).listen(port, "127.0.0.1", () => r()));

before(async () => {
  nodeSrv = createServer((req, res) => {
    let body = "";
    req.on("data", (d) => (body += d));
    req.on("end", () => {
      const q = JSON.parse(body || "{}");
      const one = (x: any) => { try { return { jsonrpc: "2.0", id: x.id, result: node(x.method, x.params) }; } catch (e) { return { jsonrpc: "2.0", id: x.id, error: { code: -32601, message: (e as Error).message } }; } };
      res.setHeader("content-type", "application/json");
      res.end(JSON.stringify(Array.isArray(q) ? q.map(one) : one(q)));
    });
  });
  await listen(nodeSrv, PORTS.node);
  dir = mkdtempSync(join(tmpdir(), "cadre-strategies-test-"));
  const cfgFile = join(dir, "strategies.json");
  writeFileSync(cfgFile, JSON.stringify({ contract: CONTRACT, start_height: 50, code_hash: "99".repeat(32), analyzer_hash: "56".repeat(32), house: [{ account: A, handle: "tape_lark", name: "Tape Lark" }] }));
  process.env.CADRE_STRATEGIES_CONFIG = cfgFile;
  const cfg = configFromEnv({ NODE_RPC: `http://127.0.0.1:${PORTS.node}`, SOLANA_RPC: `http://127.0.0.1:${PORTS.node}`, GATEWAY_URL: "off", STOREFRONT_URL: "off", POOL_URL: "off", CADRE_RATE_LIMIT_PER_MIN: "100000", CADRE_RATE_BURST: "100000" }, { stateDir: dir });
  const d = new Deps(cfg);
  d.ix.o.log = () => {};
  const api = build(d);
  api.o.log = undefined;
  apiSrv = api.listen(PORTS.api, "127.0.0.1");
  await new Promise((r) => apiSrv.on("listening", r));
  base = `http://127.0.0.1:${PORTS.api}`;
});
after(() => {
  delete process.env.CADRE_STRATEGIES_CONFIG;
  apiSrv?.close();
  nodeSrv?.close();
  rmSync(dir, { recursive: true, force: true });
});

const get = async (p: string) => {
  const r = await fetch(base + p);
  return { status: r.status, body: await r.json().catch(() => null) };
};
const postRaw = async (p: string, body: string) => {
  const r = await fetch(base + p, { method: "POST", headers: { "content-type": "application/json" }, body });
  return { status: r.status, body: await r.json().catch(() => null) };
};

test("blob store: canonical JSON of a known format only; kept by its blake3; node copies adopted", async () => {
  assert.equal((await postRaw("/v1/strategies/blobs", '{"slot":1,"format":"cadre-practice-snapshot-v1"}')).status, 400, "keys out of order");
  assert.equal((await postRaw("/v1/strategies/blobs", canonicalJson({ format: "something-else" }))).status, 400, "unknown format");
  const r = await postRaw("/v1/strategies/blobs", SNAP1);
  assert.equal(r.status, 201, JSON.stringify(r.body));
  assert.equal(r.body.id, idOf(SNAP1));
  const g = await get(`/v1/strategies/blobs/${idOf(SNAP1)}`);
  assert.equal(g.status, 200);
  assert.equal(canonicalJson(g.body), SNAP1);
  // on the node only: served and checked against its id
  const s = await get(`/v1/strategies/blobs/${idOf(SPEC1)}`);
  assert.equal(s.status, 200);
  assert.equal(s.body.name, "tape_lark v1");
  assert.equal((await get(`/v1/strategies/blobs/${"77".repeat(32)}`)).status, 404);
  assert.equal((await get("/v1/strategies/blobs/xyz")).status, 400);
});

test("board: lines from the contract's counters, house labelled, ranked by practice PnL, knowledge learned", async () => {
  const r = await get("/v1/strategies");
  assert.equal(r.status, 200, JSON.stringify(r.body));
  assert.equal(r.body.contract, CONTRACT);
  assert.equal(r.body.start_height, 50);
  assert.deepEqual(r.body.counts, { versions: 3, entries: 21, explained: 20, open: 1 });
  assert.equal(r.body.lines.length, 2);
  const [l1, l3] = r.body.lines;
  assert.equal(l1.line, 1);
  assert.equal(l1.handle, "tape_lark");
  assert.equal(l1.house, true);
  assert.equal(l1.head, 2);
  assert.equal(l1.totals.pnl_lamports, "4000000");
  assert.equal(l1.learned, 1);
  const v2 = l1.history.find((v: any) => v.id === 2);
  assert.equal(v2.knowledge.status, "learned");
  assert.equal(v2.knowledge.claimed, "RISK_MANAGEMENT");
  assert.deepEqual([v2.knowledge.baseline.faults, v2.knowledge.forward.faults, v2.knowledge.window], [6, 2, 10]);
  assert.ok(v2.knowledge.fisher_p_one_sided > 0 && v2.knowledge.fisher_p_one_sided < 0.1);
  assert.equal(l3.handle, null);
  assert.equal(l3.house, false);
  assert.equal(l3.open_entry, 21);
});

test("version, trade, knowledge and post-mortem views", async () => {
  const v = await get("/v1/strategies/versions/1");
  assert.equal(v.status, 200, JSON.stringify(v.body));
  assert.equal(v.body.spec.name, "tape_lark v1");
  assert.equal(v.body.spec_verified, true);
  assert.deepEqual(v.body.children, [2]);
  assert.equal(v.body.trades.length, 10);
  assert.equal(v.body.knowledge, null, "a first version claims nothing");
  const v2 = await get("/v1/strategies/versions/2");
  assert.deepEqual(v2.body.motives.map((t: any) => t.id), [9, 10]);
  assert.equal(v2.body.spec, null, "a spec blob nobody holds is null, never invented");
  assert.equal((await get("/v1/strategies/versions/9")).status, 404);
  assert.equal((await get("/v1/strategies/versions/x")).status, 400);

  const t = await get("/v1/strategies/trades/1");
  assert.equal(t.status, 200);
  assert.deepEqual(t.body.trade.postmortem.names, [], "a win with no fault bucket");
  assert.deepEqual((await get("/v1/strategies/trades/5")).body.trade.postmortem.names, ["RISK_MANAGEMENT"]);
  assert.equal(t.body.trade.exit.pnl, "1000000");
  assert.equal(t.body.blobs.snapshot.pool.address, "P");
  assert.equal(t.body.blobs.postmortem, null);
  const open = await get("/v1/strategies/trades/21");
  assert.equal(open.body.trade.exit, null);
  assert.equal(open.body.trade.postmortem, null);

  const k = await get("/v1/strategies/knowledge");
  assert.equal(k.body.records.length, 1);
  assert.equal(k.body.records[0].version, 2);
  assert.equal(k.body.records[0].handle, "tape_lark");
  assert.equal(k.body.min_trades, 10);

  const pm = await get("/v1/strategies/postmortems?bucket=RISK_MANAGEMENT");
  assert.equal(pm.body.total, 8);
  assert.equal(pm.body.postmortems[0].id, 20, "newest first");
  assert.equal((await get("/v1/strategies/postmortems?version=2")).body.total, 10);
  assert.equal((await get("/v1/strategies/postmortems?bucket=LUCK")).status, 400);
});

test("the trading leaderboard's strategies rows come from the same contract reads", async () => {
  assert.ok(shared.strategies, "registered when the contract is configured");
  const rows: any[] = (await shared.strategies!()) as any[];
  assert.equal(rows.length, 2);
  assert.deepEqual([rows[0].line, rows[0].head_version, rows[0].versions, rows[0].trades, rows[0].practice_pnl_lamports, rows[0].learned_fixes, rows[0].handle], [1, 2, 2, 20, "4000000", 1, "tape_lark"]);
});
