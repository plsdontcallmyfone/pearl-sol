// Node RPC load (2026-10-22, node 0 saturated by cadre-api): the shared gate caps requests in flight
// and counts them; the indexer does a full listJobs scan only every jobsFullMs (one newest page plus a
// rotating getJob refresh in between) and re-reads the top block window only when the tip moved.
import { test } from "node:test";
import assert from "node:assert/strict";
import { createServer } from "node:http";
import { mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { Indexer } from "../src/indexer.ts";
import { JsonRpc, RpcGate } from "../src/rpc.ts";
import { pendingStages } from "../src/models.ts";

const Z = "0".repeat(64);

test("gate: never more than max in flight, per-method counts", async () => {
  let live = 0, peak = 0;
  const srv = createServer((req, res) => {
    live++;
    peak = Math.max(peak, live);
    let b = "";
    req.on("data", (c) => (b += c));
    req.on("end", () => setTimeout(() => { live--; const j = JSON.parse(b); res.end(JSON.stringify(Array.isArray(j) ? j.map((x: any) => ({ id: x.id, result: 1 })) : { id: j.id, result: 1 })); }, 20));
  }).listen(10795, "127.0.0.1");
  await new Promise((r) => srv.once("listening", r));
  const gate = new RpcGate(2);
  const a = new JsonRpc("http://127.0.0.1:10795", 5000, gate), b = new JsonRpc("http://127.0.0.1:10795", 5000, gate);
  await Promise.all([...Array(6)].map((_, i) => (i % 2 ? a : b).call("getChainInfo", {})).concat([a.batch([["getBlock", {}], ["getBlock", {}]]) as any]));
  srv.close();
  assert.equal(peak, 2);
  const s = gate.snapshot();
  assert.equal(s.requests, 7);
  assert.equal(s.calls, 8);
  assert.equal(s.methods.find((m) => m.method === "getChainInfo")!.requests, 6);
  assert.ok(s.peak_queue >= 1);
});

test("indexer: one full jobs scan, then the newest page + getJob refresh; window re-read only on a new tip", async () => {
  const calls: string[] = [];
  let height = 3;
  const jobs = Array.from({ length: 450 }, (_, i) => ({ job_id: i.toString(16).padStart(64, "0"), poster: "p", status: i < 5 ? "open" : "completed", kind: "segment", protocol_fee: "1", worker_pay: "1", posted_height: 1, closed_height: 0 }));
  const node = (m: string, p: any) => {
    calls.push(m);
    switch (m) {
      case "getChainInfo": return { genesis_hash: "ab".repeat(32), height, tip: String(height).repeat(64).slice(0, 64), finality_k: 2 };
      case "getParams": return {};
      case "getBlock": return p.height > height ? null : { hash: String(p.height).repeat(64).slice(0, 64), header_fields: { height: p.height, prev_hash: Z, timestamp_ms: 1, miner: "m", job_id: Z, proof_hash: Z, target_bits: 1 }, txs: [] };
      case "listJobs": return { total: jobs.length, jobs: jobs.slice(p.offset, p.offset + p.limit) };
      case "getJob": return jobs.find((j) => j.job_id === p.job_id) ?? null;
      case "getBridgeEvents": return { withdrawals: [] };
    }
    return null;
  };
  const srv = createServer((req, res) => {
    let b = "";
    req.on("data", (c) => (b += c));
    req.on("end", () => {
      const j = JSON.parse(b);
      const one = (x: any) => ({ jsonrpc: "2.0", id: x.id, result: node(x.method, x.params) });
      res.end(JSON.stringify(Array.isArray(j) ? j.map(one) : one(j)));
    });
  }).listen(10796, "127.0.0.1");
  await new Promise((r) => srv.once("listening", r));
  const dir = mkdtempSync(join(tmpdir(), "rpcload-"));
  const ix = new Indexer({ nodeUrl: "http://127.0.0.1:10796", solUrl: "http://127.0.0.1:1", stateDir: dir, jobsFullMs: 60_000, jobsRefreshPerPoll: 3, jobsPollMs: 0, log: () => {} });
  const count = (m: string) => calls.filter((c) => c === m).length;
  await ix.pollChain();
  assert.equal(count("listJobs"), 3, "first poll: full scan, 3 pages of 200");
  assert.equal(ix.jobs.size, 450);
  calls.length = 0;
  jobs.push({ ...jobs[0], job_id: "f".repeat(64), status: "open" });
  jobs[1].status = "completed";
  await ix.pollChain();
  assert.equal(count("listJobs"), 1, "then only the newest page");
  assert.equal(count("getJob"), 3, "and a rotating refresh of open jobs");
  assert.equal(count("getBlock"), 0, "tip unchanged: no window re-read");
  assert.equal(ix.jobs.size, 451);
  assert.equal(ix.jobs.get(jobs[1].job_id).status, "completed");
  calls.length = 0;
  height = 4;
  await ix.pollChain();
  assert.ok(count("getBlock") >= 1, "new tip: the window is read");
  srv.close();
  rmSync(dir, { recursive: true, force: true });
});

test("indexer: an order's job older than the listJobs window is read by getJob and kept across full scans", async () => {
  const old = { job_id: "e".repeat(64), poster: "p", status: "open", kind: "ticket", protocol_fee: "1", worker_pay: "1", posted_height: 1, closed_height: 0 };
  const window = Array.from({ length: 5 }, (_, i) => ({ ...old, job_id: i.toString(16).padStart(64, "0"), kind: "segment" }));
  const calls: string[] = [];
  const node = (m: string, p: any) => {
    calls.push(m);
    switch (m) {
      case "getChainInfo": return { genesis_hash: "ab".repeat(32), height: 1, tip: "1".repeat(64), finality_k: 2 };
      case "getParams": return {};
      case "getBlock": return p.height > 1 ? null : { hash: "1".repeat(64), header_fields: { height: 1, prev_hash: Z, timestamp_ms: 1, miner: "m", job_id: Z, proof_hash: Z, target_bits: 1 }, txs: [] };
      case "listJobs": return { total: window.length, jobs: window.slice(p.offset, p.offset + p.limit) }; // the old job is out of the window
      case "getJob": return p.job_id === old.job_id ? old : window.find((j) => j.job_id === p.job_id) ?? null;
      case "getBridgeEvents": return { withdrawals: [] };
    }
    return null;
  };
  const srv = createServer((req, res) => {
    let b = "";
    req.on("data", (c) => (b += c));
    req.on("end", () => {
      const j = JSON.parse(b);
      const one = (x: any) => ({ jsonrpc: "2.0", id: x.id, result: node(x.method, x.params) });
      res.end(JSON.stringify(Array.isArray(j) ? j.map(one) : one(j)));
    });
  }).listen(0, "127.0.0.1"); // ephemeral port
  await new Promise((r) => srv.once("listening", r));
  const dir = mkdtempSync(join(tmpdir(), "rpcload-"));
  const ix = new Indexer({ nodeUrl: `http://127.0.0.1:${(srv.address() as any).port}`, solUrl: "http://127.0.0.1:1", stateDir: dir, jobsFullMs: 0, jobsRefreshPerPoll: 3, jobsPollMs: 0, log: () => {} });
  await ix.pollChain(); // first run: genesis recorded (a new genesis resets the orders)
  ix.orders.set("37", { nonce: "37", jobId: old.job_id, status: "pending" } as any);
  await ix.pollChain();
  assert.equal(ix.jobs.get(old.job_id)?.kind, "ticket", "fetched by getJob");
  calls.length = 0;
  await ix.pollChain(); // jobsFullMs 0: another full scan clears and refills the map
  assert.ok(ix.jobs.has(old.job_id), "kept across the full scan");
  assert.equal(calls.filter((c) => c === "getJob").length, 0, "not fetched again");
  ix.orders.set("38", { nonce: "38", jobId: "d".repeat(64), status: "pending" } as any);
  await ix.pollChain();
  assert.deepEqual(pendingStages(ix), { not_unlocked: 1, job_open: 1, awaiting_light_client: 0, settleable: 0, job_unknown: 0 });
  srv.close();
  rmSync(dir, { recursive: true, force: true });
});
