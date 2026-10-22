// Tx status (docs/SPEED.md 2.4; docs/API.md "Tx status") against a fake node: a weak-block
// node (getTxStatus) and an older node (getTx only, final derived from the finalized height).
// Ports 10686 (node) and 10689 (api), the owned test block; files run one at a time.
import { test, before, after } from "node:test";
import assert from "node:assert/strict";
import { createServer, type Server } from "node:http";
import { mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { configFromEnv, Deps } from "../src/deps.ts";
import { build } from "../src/app.ts";

const PORTS = { node: 10686, api: 10689 };
const h = (b: string) => b.repeat(32);
const PENDING = h("01"), PRE = h("02"), INC = h("03"), OLDFINAL = h("04");
let weakNode = true;

function node(m: string, p: any): any {
  switch (m) {
    case "getChainInfo": return { chain_id: 7, genesis_hash: h("cd"), height: 100, finalized_height: 70, finality_k: 30 };
    case "getParams": return { min_fee: "1000", finality_k: 30, target_block_time_ms: 60000 };
    case "getBlock": return null;
    case "listJobs": return { total: 0, jobs: [] };
    case "getBridgeEvents": return { withdrawals: [] };
    case "getTxStatus": {
      if (!weakNode) throw new Error("unknown method getTxStatus");
      if (p.tx_hash === PENDING) return { status: "pending", final: false };
      if (p.tx_hash === PRE) return { status: "preconfirmed", final: false, preconf: { weak_count: 3, weak_miners: 2, weak_share: 0.8, first_seen_ms: 1792617000000, latest_age_ms: 1400, confidence: "medium" } };
      if (p.tx_hash === INC) return { status: "included", final: false, block_hash: h("aa"), height: 95, confirmations: 6 };
      return null;
    }
    case "getTx": {
      if (p.tx_hash === PRE) return weakNode ? { tx_hash: PRE, kind: "Transfer", status: "preconfirmed", preconf: { weak_count: 1, weak_miners: 1, weak_share: 1, latest_age_ms: 300, confidence: "low" } } : { tx_hash: PRE, kind: "Transfer", status: "pending" };
      if (p.tx_hash === OLDFINAL) return { tx_hash: OLDFINAL, kind: "Transfer", status: "included", block_hash: h("bb"), height: 60, confirmations: 41 };
      if (p.tx_hash === INC) return { tx_hash: INC, kind: "Transfer", status: "included", block_hash: h("aa"), height: 95, confirmations: 6 };
      return null;
    }
    default: throw new Error(`unknown method ${m}`);
  }
}

const rpcServer = () =>
  createServer((req, res) => {
    let body = "";
    req.on("data", (d) => (body += d));
    req.on("end", () => {
      const q = JSON.parse(body || "{}");
      const one = (x: any) => {
        try {
          return { jsonrpc: "2.0", id: x.id, result: node(x.method, x.params) };
        } catch (e) {
          return { jsonrpc: "2.0", id: x.id, error: { code: -32601, message: (e as Error).message } };
        }
      };
      res.setHeader("content-type", "application/json");
      res.end(JSON.stringify(Array.isArray(q) ? q.map(one) : one(q)));
    });
  });

let nodeSrv: Server, apiSrv: Server, dir = "", base = "";
const listen = (s: Server, port: number) => new Promise<void>((r, j) => s.once("error", j).listen(port, "127.0.0.1", () => r()));

before(async () => {
  nodeSrv = rpcServer();
  await listen(nodeSrv, PORTS.node);
  dir = mkdtempSync(join(tmpdir(), "cadre-txstatus-test-"));
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
  apiSrv?.close();
  nodeSrv?.close();
  rmSync(dir, { recursive: true, force: true });
});

const get = async (p: string) => {
  const r = await fetch(base + p);
  return { status: r.status, body: await r.json().catch(() => null) };
};

test("status route on a weak-block node: pending, preconfirmed with evidence, included N of K", async () => {
  const p = await get(`/v1/tx/chain/${PENDING}/status`);
  assert.equal(p.status, 200);
  assert.equal(p.body.status, "pending");
  assert.equal(p.body.node_api, "getTxStatus");
  const pre = await get(`/v1/tx/chain/${PRE}/status`);
  assert.equal(pre.body.status, "preconfirmed");
  assert.deepEqual([pre.body.preconf.weak_count, pre.body.preconf.weak_miners, pre.body.preconf.confidence], [3, 2, "medium"]);
  assert.match(pre.body.risk, /Not binding/);
  const inc = await get(`/v1/tx/chain/${INC}/status`);
  assert.equal(inc.body.status, "included");
  assert.equal(inc.body.confirmations, 6);
  assert.equal(inc.body.finality_k, 30);
  assert.equal(inc.body.final, false);
  assert.equal((await get(`/v1/tx/chain/${h("ee")}/status`)).status, 404);
  assert.equal((await get(`/v1/tx/chain/xyz/status`)).status, 400);
});

test("the tx route keeps the node's fields and adds tx_status", async () => {
  const r = await get(`/v1/tx/chain/${PRE}`);
  assert.equal(r.body.status, "preconfirmed");
  assert.equal(r.body.tx_status.status, "preconfirmed");
  assert.equal(r.body.tx_status.preconf.confidence, "low");
  const f = await get(`/v1/tx/chain/${OLDFINAL}`);
  assert.equal(f.body.status, "included", "the node's own field is unchanged");
  assert.equal(f.body.tx_status.status, "final");
});

test("an older node without getTxStatus: falls back to getTx, final derived", async () => {
  weakNode = false;
  await new Promise((r) => setTimeout(r, 1100)); // past the 1 s response cache of the earlier tests
  try {
    const f = await get(`/v1/tx/chain/${OLDFINAL}/status`);
    assert.equal(f.status, 200);
    assert.equal(f.body.node_api, "getTx");
    assert.equal(f.body.status, "final");
    assert.equal(f.body.final_source, "derived");
    const pre = await get(`/v1/tx/chain/${PRE}/status`);
    assert.equal(pre.body.status, "pending");
    assert.equal(pre.body.preconf, null);
  } finally {
    weakNode = true;
  }
});
