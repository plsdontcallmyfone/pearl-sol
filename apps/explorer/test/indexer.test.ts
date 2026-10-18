// Indexer against an in-process fake node + fake Solana RPC: indexing, reorg below the
// tip, a restarted chain (new genesis), persistence, and the HTTP routes on top.
import { test } from "node:test";
import assert from "node:assert/strict";
import { createServer } from "node:http";
import { mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { Indexer } from "../src/indexer.ts";
import { route } from "../src/server.ts";

const Z = "0".repeat(64);
const h64 = (s: string) => (s.repeat(64)).slice(0, 64);

interface Chain {
  genesis: string;
  blocks: { hash: string; job: string | null; subsidy: string }[]; // index = height - 1
  jobs: any[];
}

function fakeNode(chain: () => Chain) {
  const block = (h: number) => {
    const c = chain();
    const b = c.blocks[h - 1];
    if (!b) return null;
    return {
      hash: b.hash, fees: "1000", subsidy: b.subsidy, reward: "1000", proof_len: 100, canonical: true, final: h <= c.blocks.length - 2, confirmations: c.blocks.length - h,
      header_fields: { height: h, prev_hash: h > 1 ? c.blocks[h - 2].hash : Z, timestamp_ms: 1_700_000_000_000 + h * 1500, miner: "Miner111111111111111111111111111111111111111", job_id: b.job ?? Z, proof_hash: h64("p"), target_bits: 1 },
      txs: [],
    };
  };
  const handle = (m: string, p: any) => {
    const c = chain();
    switch (m) {
      case "getChainInfo": return { genesis_hash: c.genesis, height: c.blocks.length, finalized_height: c.blocks.length - 2, finality_k: 2 };
      case "getParams": return { alpha_ppm: 800000, emission_ppm: 10000, min_fee: "1000" };
      case "getBlock": return block(p.height);
      case "listJobs": return { total: c.jobs.length, jobs: c.jobs };
      case "getBridgeEvents": return { withdrawals: [] };
      default: throw new Error("no " + m);
    }
  };
  return createServer((req, res) => {
    let body = "";
    req.on("data", (d) => (body += d));
    req.on("end", () => {
      const q = JSON.parse(body);
      const one = (x: any) => {
        try { return { jsonrpc: "2.0", id: x.id, result: handle(x.method, x.params) }; } catch (e) { return { jsonrpc: "2.0", id: x.id, error: { message: (e as Error).message } }; }
      };
      res.end(JSON.stringify(Array.isArray(q) ? q.map(one) : one(q)));
    });
  });
}

function fakeSolana() {
  return createServer((req, res) => {
    let body = "";
    req.on("data", (d) => (body += d));
    req.on("end", () => {
      const q = JSON.parse(body);
      let result: any = null;
      if (q.method === "getMultipleAccounts") result = { value: q.params[0].map(() => null) };
      if (q.method === "getProgramAccounts") result = [];
      if (q.method === "getSignaturesForAddress") result = [];
      res.end(JSON.stringify({ jsonrpc: "2.0", id: q.id, result }));
    });
  });
}

const listen = (s: ReturnType<typeof createServer>) => new Promise<number>((r) => s.listen(0, "127.0.0.1", () => r((s.address() as any).port)));

const job = (id: string, posted: number) => ({ job_id: id, poster: "Poster1111111111111111111111111111111111111", status: "open", worker_pay: "400", protocol_fee: "100", subsidy_paid: "80", cap_remaining: "0", blocks_mined: 2, max_blocks: 5, posted_height: posted, closed_height: 0, deadline_height: 999, index: 0, hash_a: Z, data_uri: "", evaluators: [], threshold: 1 });

test("indexer: index, reorg, restart with a new genesis, persistence, routes", async () => {
  const J = h64("a1");
  let c: Chain = {
    genesis: h64("g1"),
    blocks: Array.from({ length: 12 }, (_, i) => ({ hash: h64(`b${i + 1}x`), job: i + 1 === 5 || i + 1 === 6 ? J : null, subsidy: i + 1 === 5 || i + 1 === 6 ? "40" : "0" })),
    jobs: [job(J, 4)],
  };
  const node = fakeNode(() => c);
  const sol = fakeSolana();
  const [np, sp] = await Promise.all([listen(node), listen(sol)]);
  const dir = mkdtempSync(join(tmpdir(), "explorer-test-"));
  const mk = () => new Indexer({ nodeUrl: `http://127.0.0.1:${np}`, solUrl: `http://127.0.0.1:${sp}`, stateDir: dir, log: () => {} });
  try {
    const ix = mk();
    await ix.poll();
    assert.equal(ix.blocks.size, 12);
    assert.deepEqual(ix.jobBlocks.get(J), [5, 6]);

    const sum = JSON.parse((await route(ix, "GET", "/api/summary")).body as string);
    assert.deepEqual(sum.blocks, { total: 12, paid_job_blocks: 2, fallback_blocks: 10, paid_share: "16.7%" });
    assert.equal(sum.fees.subsidy_paid, "80");
    assert.equal(sum.fees.protocol_fees_paid, "100");
    assert.equal(sum.payers.unique_payers, 1);

    const feed = JSON.parse((await route(ix, "GET", "/api/feed")).body as string);
    assert.deepEqual(feed.rows.map((r: any) => (r.kind === "block" ? r.block.height : `${r.to}-${r.from}`)), ["12-7", 6, 5, "4-1"]);
    const b5 = JSON.parse((await route(ix, "GET", "/api/block/5")).body as string);
    assert.equal(b5.kind, "paid_job");
    assert.equal(b5.subsidy_cap, "80");
    assert.equal(b5.job_subsidy_after_block, "40");
    assert.equal(b5.within_cap, true);
    assert.equal(b5.light_client.state, "none");
    const jp = JSON.parse((await route(ix, "GET", `/api/job/${J.slice(0, 8)}`)).body as string);
    assert.deepEqual(jp.blocks.map((b: any) => [b.height, b.job_subsidy_after_block]), [[5, "40"], [6, "80"]]);
    for (const p of ["/", "/blocks?filter=paid", "/block/5", "/block/9", `/job/${J}`, "/jobs", "/orders", "/docs"]) {
      const r = await route(ix, "GET", p);
      assert.equal(r.status, 200, p);
      assert.match(r.body as string, /<!doctype html>/);
    }
    assert.equal((await route(ix, "GET", "/block/999")).status, 404);
    assert.equal((await route(ix, "POST", "/api/summary")).status, 405);

    // reorg: the last two blocks are replaced, block 11 now mines the job
    c = { ...c, blocks: [...c.blocks.slice(0, 10), { hash: h64("r11"), job: J, subsidy: "0" }, { hash: h64("r12"), job: null, subsidy: "0" }, { hash: h64("r13"), job: null, subsidy: "0" }] };
    await ix.poll();
    assert.equal(ix.blocks.size, 13);
    assert.equal(ix.blocks.get(11)!.hash, h64("r11"));
    assert.equal(ix.byHash.has(h64("b11x")), false);
    assert.deepEqual(ix.jobBlocks.get(J), [5, 6, 11]);

    ix.save(true);
    // a second indexer on the same genesis loads the saved index
    const ix2 = mk();
    await ix2.poll();
    assert.equal(ix2.blocks.get(11)!.hash, h64("r11"));

    // the stack restarts: new genesis, shorter chain -> everything re-indexed
    c = { genesis: h64("g2"), blocks: Array.from({ length: 3 }, (_, i) => ({ hash: h64(`n${i}`), job: null, subsidy: "0" })), jobs: [] };
    await ix2.poll();
    assert.equal(ix2.genesis, h64("g2"));
    assert.equal(ix2.blocks.size, 3);
    assert.equal(ix2.jobs.size, 0);
    assert.equal(ix2.status.resets, 2);
  } finally {
    node.close();
    sol.close();
    rmSync(dir, { recursive: true, force: true });
  }
});
