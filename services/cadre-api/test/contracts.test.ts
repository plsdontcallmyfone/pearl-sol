// Contract routes (docs/API.md "Contracts") against a fake node serving the contract RPC with
// the fixture records of apps/explorer/test/contract-fixture.ts (real vector tx bytes and
// receipt), plus a node without the contract RPC. Ports 10686 (node) and 10689 (api), the
// owned test block; files run one at a time (--test-concurrency=1).
import { test, before, after } from "node:test";
import assert from "node:assert/strict";
import { createServer, type Server } from "node:http";
import { mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { configFromEnv, Deps } from "../src/deps.ts";
import { build } from "../src/app.ts";
import { hex } from "../../../apps/explorer/src/codec.ts";
import { contractSigningBytes, encodeCallBody, eventTopic, framedMessage } from "../../../apps/explorer/src/contracts.ts";
import { CALL, CODE, COUNTER, PARAMS, counter, counterEvents, receipt } from "../../../apps/explorer/test/contract-fixture.ts";

const PORTS = { node: 10686, api: 10689 };
const calls: { m: string; p: any }[] = [];
let contractsOn = true;
let params: any = PARAMS;
const RICH = CALL.from;
const POOR = "11111111111111111111111111111112";

function node(m: string, p: any): any {
  calls.push({ m, p });
  const cm = ["listContracts", "listCodes", "getContract", "getCode", "getContractEvents", "getContractReceipt", "queryContract", "simulateCall"];
  if (!contractsOn && cm.includes(m)) throw new Error(`unknown method ${m}`);
  switch (m) {
    case "getChainInfo": return { chain_id: CALL.chain_id, genesis_hash: "cd".repeat(32), height: 3, finalized_height: 1, finality_k: 2 };
    case "getParams": return params;
    case "getBlock": return null;
    case "listJobs": return { total: 0, jobs: [] };
    case "getBridgeEvents": return { withdrawals: [] };
    case "getAccount": return { account: p.account, balance: p.account === RICH ? "5000000000" : "1000", nonce: p.account === RICH ? CALL.nonce : 0, route: null };
    case "listContracts": return { total: 1, offset: p.offset, items: [counter] };
    case "listCodes": return { total: 1, offset: p.offset, items: [{ code_hash: CODE, deployer: RICH, height: 11, size: 8, deposit: "8000", instances: 1, index: 0 }] };
    case "getContract": return p.address === COUNTER ? counter : null;
    case "getCode": return p.code_hash === CODE ? { code_hash: CODE, deployer: RICH, height: 11, size: 8, deposit: "8000", instances: 1, index: 0, ...(p.include_code ? { code: "0061736d01000000" } : {}) } : null;
    case "getContractEvents": {
      if (p.address !== COUNTER) return null;
      const l = counterEvents.filter((e) => !p.topic || e.topics[0] === p.topic);
      return { address: COUNTER, total: counterEvents.length, events: l, next_offset: l.length };
    }
    case "getContractReceipt": return p.tx_hash === CALL.tx_hash ? receipt : null;
    case "queryContract": return { ok: true, return: "0800000000000000", error: "", gas_used: "1200", height: 3 };
    case "simulateCall": return { ok: false, return: "", error: "reverted: not the owner", gas_used: "900", log: "", events: [{ ...counterEvents[1] }], fee_estimate: "101500", height: 3 };
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
  dir = mkdtempSync(join(tmpdir(), "cadre-contracts-test-"));
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
const post = async (p: string, b: unknown) => {
  const r = await fetch(base + p, { method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify(b) });
  return { status: r.status, body: await r.json().catch(() => null) };
};
const last = (m: string) => [...calls].reverse().find((c) => c.m === m)!.p;

test("list, one contract, events with names and a topic filter by name, receipts, codes", async () => {
  const l = await get("/v1/contracts?limit=10");
  assert.equal(l.status, 200);
  assert.equal(l.body.items[0].address, COUNTER);
  assert.equal(last("listContracts").limit, 10);
  assert.equal((await get("/v1/contracts?limit=500")).status, 400);
  const c = await get(`/v1/contracts/${COUNTER}`);
  assert.equal(c.body.label, "counter");
  assert.equal(c.body.upgradable, true);
  assert.equal(c.body.balance, "1000000000");
  assert.deepEqual(c.body.entries, counter.entries);
  assert.equal((await get(`/v1/contracts/${POOR}`)).status, 404);
  assert.equal((await get("/v1/contracts/nope")).status, 400);
  const e = await get(`/v1/contracts/${COUNTER}/events`);
  assert.deepEqual(e.body.events.map((x: any) => x.name), ["Incremented", "Reset", null]);
  assert.equal(e.body.total, 3);
  const f = await get(`/v1/contracts/${COUNTER}/events?topic=Reset&limit=5`);
  assert.equal(last("getContractEvents").topic, eventTopic("Reset"));
  assert.equal(f.body.topic_name, "Reset");
  assert.deepEqual(f.body.events.map((x: any) => x.name), ["Reset"]);
  const r = await get(`/v1/contracts/receipts/${CALL.tx_hash}`);
  assert.equal(r.body.status, "ok");
  assert.equal(r.body.gas_used, "412345");
  assert.equal(r.body.events[0].name, "Incremented");
  assert.equal((await get(`/v1/contracts/receipts/${"00".repeat(32)}`)).status, 404);
  const codes = await get("/v1/codes");
  assert.equal(codes.body.items[0].code_hash, CODE);
  const code = await get(`/v1/codes/${CODE}?include_code=1`);
  assert.equal(code.body.code, "0061736d01000000");
  assert.equal(last("getCode").include_code, true);
  assert.equal((await get(`/v1/codes/${"ab".repeat(32)}`)).status, 404);
});

test("query: method + fields encoded to the vector's args, return decoded on request; simulate passes from and value", async () => {
  const q = await post(`/v1/contracts/${COUNTER}/query`, { method: "increment", fields: [{ u64: "5" }], returns: ["u64"] });
  assert.equal(q.status, 200);
  assert.equal(last("queryContract").args, "09696e6372656d656e740500000000000000");
  assert.equal(q.body.return, "0800000000000000");
  assert.deepEqual(q.body.decoded, ["8"]);
  assert.equal(q.body.method, "increment");
  const q2 = await post(`/v1/contracts/${COUNTER}/query`, { args: "0367657400", gas: "5000", returns: ["u32"] });
  assert.equal(last("queryContract").gas, "5000");
  assert.equal(q2.body.decoded, null);
  assert.match(q2.body.decode_error, /does not decode as u32/);
  assert.equal((await post(`/v1/contracts/${COUNTER}/query`, { args: "00", method: "x" })).status, 400);
  assert.equal((await post(`/v1/contracts/${COUNTER}/query`, { method: "x", fields: ["u8:300"] })).status, 400);
  assert.equal((await post(`/v1/contracts/${COUNTER}/query`, {})).status, 400);
  const s = await post(`/v1/contracts/${COUNTER}/simulate`, { from: RICH, method: "reset", value: "7" });
  assert.equal(s.status, 200);
  assert.equal(last("simulateCall").from, RICH);
  assert.equal(last("simulateCall").value, "7");
  assert.equal(s.body.ok, false);
  assert.equal(s.body.error, "reverted: not the owner");
  assert.equal(s.body.events[0].name, "Reset");
  assert.equal((await post(`/v1/contracts/${COUNTER}/simulate`, { method: "reset" })).status, 400, "from is required");
});

test("unsigned contract-call: bytes equal the vector's signing bytes, fee floor from getParams, balance and gas checks", async () => {
  const b = { from: CALL.from, contract: CALL.body.contract, value: CALL.body.value, gas_limit: CALL.body.gas_limit, args: CALL.body.args };
  const r = await post("/v1/tx/contract-call", b);
  assert.equal(r.status, 200, JSON.stringify(r.body));
  const len = 54 + encodeCallBody({ contract: b.contract, value: b.value, gasLimit: b.gas_limit, args: Buffer.from(b.args, "hex") }).length + 64;
  assert.equal(r.body.tx_len, len);
  // 100000 + 1000 x len + ceil(1000000 x 1500 / 1e6)
  const floor = 100000n + 1000n * BigInt(len) + 1500n;
  assert.equal(r.body.fee, floor.toString());
  assert.equal(r.body.fee_floor, floor.toString());
  assert.equal(r.body.fee_parts.gas_fee, "1500");
  assert.equal(r.body.nonce, CALL.nonce);
  const t = r.body.window_pearl.params.transaction;
  assert.deepEqual(t, { kind: "Call", contract: COUNTER, value: CALL.body.value, gasLimit: CALL.body.gas_limit, args: CALL.body.args, fee: floor.toString() });
  assert.equal(r.body.summary.method, "increment");
  assert.equal(r.body.summary.label, "counter");
  // with the vector's fee the bytes are the vector's signing bytes and framed message
  const v = await post("/v1/tx/contract-call", { ...b, fee: "2000000" });
  assert.equal(v.body.unsigned_tx, CALL.signing_bytes);
  assert.equal(v.body.framed_message, CALL.framed_message);
  assert.equal(r.body.unsigned_tx, hex(contractSigningBytes(CALL.chain_id, CALL.from, CALL.nonce, floor, "Call", encodeCallBody({ contract: b.contract, value: b.value, gasLimit: b.gas_limit, args: Buffer.from(b.args, "hex") }))));
  assert.equal(r.body.framed_message, hex(framedMessage(Buffer.from(r.body.unsigned_tx, "hex"))));
  // method + fields build the same args
  const mf = await post("/v1/tx/contract-call", { ...b, args: undefined, method: "increment", fields: ["u64:5"] });
  assert.equal(mf.body.window_pearl.params.transaction.args, CALL.body.args);
  // refusals
  const low = await post("/v1/tx/contract-call", { ...b, fee: "1000" });
  assert.equal(low.status, 400);
  assert.match(low.body.error.message, /below the floor/);
  assert.equal((await post("/v1/tx/contract-call", { ...b, from: POOR })).status, 422);
  assert.equal((await post("/v1/tx/contract-call", { ...b, contract: POOR })).status, 404);
  assert.equal((await post("/v1/tx/contract-call", { ...b, gas_limit: "60000000" })).status, 400, "above tx_gas_limit");
  assert.equal((await post("/v1/tx/contract-call", { ...b, gas_limit: undefined })).status, 400);
});

test("a node without the contract RPC: 503 unavailable with the reason", async () => {
  contractsOn = false;
  params = { min_fee: "1000" };
  try {
    const l = await get("/v1/contracts?offset=1");
    assert.equal(l.status, 503);
    assert.equal(l.body.error.code, "unavailable");
    assert.match(l.body.error.message, /does not serve listContracts/);
    assert.equal((await get(`/v1/contracts/${COUNTER}/events?offset=1`)).status, 503);
    const q = await post(`/v1/contracts/${COUNTER}/query`, { method: "get" });
    assert.equal(q.status, 503);
  } finally {
    contractsOn = true;
    params = PARAMS;
  }
});
