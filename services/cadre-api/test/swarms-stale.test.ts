// POST /v1/tx/swarm accept (audit follow-up 2026-10-23, docs/SWARMS.md): the deployed swarms
// contract still accepts a proposal whose lead has left or handed the lead on, so the API builds
// an accept only for an open proposal of the swarm's current lead, and GET /v1/swarms/:id labels
// each proposal's standing. Fake node and API on OS-assigned ports (listen 0).
import { test, before, after } from "node:test";
import assert from "node:assert/strict";
import { createServer, type Server } from "node:http";
import type { AddressInfo } from "node:net";
import { mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { configFromEnv, Deps } from "../src/deps.ts";
import { Api } from "../src/http.ts";
import { registerSwarms } from "../src/routes/swarms.ts";
import { b58decode, b58encode } from "../../../apps/explorer/src/codec.ts";

const key = (i: number) => b58encode(Uint8Array.from({ length: 32 }, (_, j) => (j === 0 ? i : 7)));
const CONTRACT = key(1), CREATOR = key(2), LEAD = key(3), NEW_LEAD = key(4), AGENT = key(5);

class W {
  p: Buffer[] = [];
  u8(v: number) { this.p.push(Buffer.from([v])); return this; }
  u32(v: number) { const b = Buffer.alloc(4); b.writeUInt32LE(v); this.p.push(b); return this; }
  u64(v: number | bigint) { const b = Buffer.alloc(8); b.writeBigUInt64LE(BigInt(v)); this.p.push(b); return this; }
  addr(a: string | null) { this.p.push(a ? Buffer.from(b58decode(a)) : Buffer.alloc(32)); return this; }
  h(x: string) { this.p.push(Buffer.from(x, "hex")); return this; }
  s(t: string) { const b = Buffer.from(t); this.u32(b.length); this.p.push(b); return this; }
  hex() { return Buffer.concat(this.p).toString("hex"); }
}
// swarm 1 led by LEAD; swarm 2 lost its lead (LEAD left); swarm 3 is led by NEW_LEAD now
const leads: Record<number, string | null> = { 1: LEAD, 2: null, 3: NEW_LEAD };
const swarmHex = (id: number) => new W().addr(CREATOR).addr(leads[id]).addr(null).h("11".repeat(32)).h("22".repeat(32)).u64(50).u32(2).u32(1).u32(1).s(`crew ${id}`).s("blob:goal").hex();
// proposal n belongs to swarm n, all made by LEAD for AGENT, open
const proposalHex = (n: number) => new W().u64(n).addr(AGENT).addr(LEAD).u8(0).u64(60).u64(0).s("scout").hex();

function rpc(m: string, p: any): any {
  switch (m) {
    case "getChainInfo": return { chain_id: 7, genesis_hash: "ab".repeat(32), height: 140, finalized_height: 100, finality_k: 30 };
    case "getParams": return { min_fee: "1000", contract_base_fee: "1000", contract_byte_fee: "10", gas_price: "10000", gas_price_unit: "1000000", tx_gas_limit: "50000000" };
    case "getAccount": return { balance: "100000000000", nonce: 3 };
    case "simulateCall": return { ok: true, gas_used: 700_000 }; // the contract itself would accept every one of them
    case "getBlock": return null;
    case "listJobs": return { total: 0, jobs: [] };
    case "getBridgeEvents": return { withdrawals: [] };
    case "queryContract": {
      const a = Buffer.from(p.args, "hex");
      const meth = a.subarray(1, 1 + a[0]).toString();
      const arg = a.length >= 1 + a[0] + 8 ? Number(a.readBigUInt64LE(1 + a[0])) : 0;
      if (meth === "swarm") return { ok: true, return: swarmHex(arg) };
      if (meth === "proposal") return { ok: true, return: proposalHex(arg) };
      if (meth === "proposals") return { ok: true, return: new W().u32(1).u8(1).u64(arg).hex() };
      if (meth === "members") return { ok: true, return: new W().u32(0).u8(0).hex() };
      return { ok: false, error: "" };
    }
    default: throw new Error(`unknown method ${m}`);
  }
}

let rpcSrv: Server, apiSrv: Server, dir = "", base = "";
before(async () => {
  rpcSrv = createServer((req, res) => {
    let body = "";
    req.on("data", (d) => (body += d));
    req.on("end", () => {
      const q = JSON.parse(body || "{}");
      const one = (x: any) => { try { return { jsonrpc: "2.0", id: x.id, result: rpc(x.method, x.params) }; } catch (e) { return { jsonrpc: "2.0", id: x.id, error: { code: -32601, message: (e as Error).message } }; } };
      res.setHeader("content-type", "application/json");
      res.end(JSON.stringify(Array.isArray(q) ? q.map(one) : one(q)));
    });
  });
  await new Promise<void>((r) => rpcSrv.listen(0, "127.0.0.1", () => r()));
  const rpcPort = (rpcSrv.address() as AddressInfo).port;
  dir = mkdtempSync(join(tmpdir(), "cadre-swarms-stale-"));
  const cfgFile = join(dir, "swarms.json");
  writeFileSync(cfgFile, JSON.stringify({ contract: CONTRACT, start_height: 0 }));
  const cfg = configFromEnv({ NODE_RPC: `http://127.0.0.1:${rpcPort}`, SOLANA_RPC: `http://127.0.0.1:${rpcPort}`, GATEWAY_URL: "off", STOREFRONT_URL: "off", POOL_URL: "off" }, { stateDir: dir });
  const d = new Deps(cfg);
  d.ix.o.log = () => {};
  const api = new Api({ allowedOrigins: "*", rateLimitPerMin: 0, rateBurst: 1000, trustProxy: false, maxBody: 1 << 20 });
  registerSwarms(api, d, { CADRE_SWARMS_CONFIG: cfgFile });
  api.o.log = undefined;
  apiSrv = api.listen(0, "127.0.0.1");
  await new Promise((r) => apiSrv.on("listening", r));
  base = `http://127.0.0.1:${(apiSrv.address() as AddressInfo).port}`;
});
after(() => { apiSrv?.close(); rpcSrv?.close(); rmSync(dir, { recursive: true, force: true }); });
const post = async (p: string, b: unknown) => { const r = await fetch(base + p, { method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify(b) }); return { status: r.status, body: await r.json().catch(() => null) }; };

test("accept is built only for an open proposal of the swarm's current lead", async () => {
  const ok = await post("/v1/tx/swarm", { from: AGENT, action: "accept", proposal: 1 });
  assert.equal(ok.status, 200, JSON.stringify(ok.body));
  assert.ok(ok.body.unsigned_tx);
  const gone = await post("/v1/tx/swarm", { from: AGENT, action: "accept", proposal: 2 });
  assert.equal(gone.status, 409, JSON.stringify(gone.body));
  assert.equal(gone.body.error.code, "stale_proposal");
  assert.equal(gone.body.error.details.standing, "no_lead");
  assert.match(gone.body.error.message, /decline it/);
  const moved = await post("/v1/tx/swarm", { from: AGENT, action: "accept", proposal: 3 });
  assert.equal(moved.status, 409);
  assert.equal(moved.body.error.details.standing, "lead_changed");
  // declining a stale proposal is still built (it closes it on the record)
  assert.equal((await post("/v1/tx/swarm", { from: AGENT, action: "decline", proposal: 2 })).status, 200);
});

test("the swarm page labels each proposal's standing", async () => {
  const r = await fetch(base + "/v1/swarms/2");
  const b = await r.json();
  assert.equal(r.status, 200, JSON.stringify(b));
  assert.equal(b.proposals_list[0].standing, "no_lead");
  const r1 = await (await fetch(base + "/v1/swarms/1")).json();
  assert.equal(r1.proposals_list[0].standing, "valid");
});
