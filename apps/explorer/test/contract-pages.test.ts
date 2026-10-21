// Contract pages and their /api twins (src/contract-route.ts, contract-view.ts) over a real
// indexer on the fake node of test/contract-fixture.ts (port 10692, in the explorer test block).
// Also a node without the contract RPC: every page says "Unavailable" with the reason.
import { test, before, after } from "node:test";
import assert from "node:assert/strict";
import { mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import type { Server } from "node:http";
import { Indexer } from "../src/indexer.ts";
import { route } from "../src/server.ts";
import { eventTopic } from "../src/contracts.ts";
import { CALL, CODE, COUNTER, ESCROW, INST, OTHER_TX, fakeNode } from "./contract-fixture.ts";

const PORT = 10692;
let node: Server, ix: Indexer, dir = "";
const get = async (url: string) => {
  const r = await route(ix, "GET", url);
  return { ...r, text: String(r.body), json: () => JSON.parse(String(r.body)) };
};
const dash = String.fromCharCode(0x2014);

before(async () => {
  node = fakeNode();
  await new Promise<void>((r, j) => node.once("error", j).listen(PORT, "127.0.0.1", () => r()));
  dir = mkdtempSync(join(tmpdir(), "explorer-contracts-"));
  ix = new Indexer({ nodeUrl: `http://127.0.0.1:${PORT}`, solUrl: `http://127.0.0.1:${PORT}`, stateDir: dir, log: () => {} });
  await ix.poll().catch(() => {});
});
after(() => {
  ix?.stop();
  node?.close();
  rmSync(dir, { recursive: true, force: true });
});

test("contracts list: newest first, codes, gas params; nav entry", async () => {
  const j = (await get("/api/contracts")).json();
  assert.equal(j.contracts.ok, true);
  assert.deepEqual(j.contracts.v.items.map((c: any) => c.label), ["escrow", "counter"]);
  assert.equal(j.codes.v.total, 2);
  assert.equal(j.params.gas_price, "1500");
  const h = await get("/contracts");
  assert.equal(h.status, 200);
  assert.match(h.text, /href="\/contract\/[1-9A-Za-z]+"/);
  assert.match(h.text, /immutable/);
  assert.match(h.text, /upgradable/);
  assert.match(h.text, /aria-current="page">Contracts</);
  assert.match(h.text, /1,500<\/span> chain units per <span class="num">1,000,000<\/span> gas/);
  assert.equal(h.text.includes(dash), false);
});

test("contract page: record, admin, storage, events with decoded topic names, topic filter by name", async () => {
  const j = (await get(`/api/contract/${COUNTER}`)).json();
  assert.equal(j.contract.v.label, "counter");
  assert.deepEqual(j.events.v.events.map((e: any) => e.name), ["Incremented", "Reset", null]);
  const h = await get(`/contract/${COUNTER}`);
  assert.equal(h.status, 200);
  for (const s of ["counter", "upgradable", `/code/${CODE}`, "Incremented", "Reset", `/tx/${CALL.tx_hash}`, "48</span> B", "instantiate, call, query, migrate"]) assert.ok(h.text.includes(s), s);
  assert.equal(h.text.includes(dash), false);
  const f = (await get(`/api/contract/${COUNTER}?topic=Reset`)).json();
  assert.equal(f.topic, eventTopic("Reset"));
  assert.deepEqual(f.events.v.events.map((e: any) => e.name), ["Reset"]);
  const imm = await get(`/contract/${ESCROW}`);
  assert.match(imm.text, /no admin: it can never be migrated/);
  assert.match(imm.text, /no events yet/);
  assert.equal((await get(`/api/contract/${INST.from}`)).status, 404);
  assert.equal((await get("/api/contract/not-an-address")).status, 400);
});

test("code page: record and the contracts running it", async () => {
  const j = (await get(`/api/code/${CODE}`)).json();
  assert.equal(j.code.v.size, 8);
  assert.deepEqual(j.instances.v.items.map((c: any) => c.address), [COUNTER]);
  const h = await get(`/code/${CODE}`);
  assert.equal(h.status, 200);
  assert.ok(h.text.includes(`/contract/${COUNTER}`));
  assert.equal((await get(`/api/code/${"00".repeat(32)}`)).status, 404);
});

test("tx page: a Call with its receipt (status, gas used, fee charged, events); a pending Instantiate; block page links and the contracts record", async () => {
  const j = (await get(`/api/tx/${CALL.tx_hash}`)).json();
  assert.equal(j.contract.kind, "Call");
  assert.equal(j.contract.body.contract, COUNTER);
  assert.equal(j.contract.body.method, "increment");
  assert.equal(j.receipt.v.status, "ok");
  assert.equal(j.receipt.v.events[0].name, "Incremented");
  const h = await get(`/tx/${CALL.tx_hash}`);
  for (const s of ["Receipt", "412,345", "1,000,000", "increment", "Incremented", "0.001497", "Call body"]) assert.ok(h.text.includes(s), s);
  const p = (await get(`/api/tx/${INST.tx_hash}`)).json();
  assert.equal(p.receipt.ok, false);
  assert.match(p.receipt.reason, /pending in the mempool/);
  assert.equal(p.contract.body.contract, COUNTER, "the address an Instantiate will create");
  assert.equal((await get(`/api/tx/${OTHER_TX}`)).status, 404);
  const b = await get("/block/24");
  assert.equal(b.status, 200);
  assert.ok(b.text.includes(`/tx/${CALL.tx_hash}`));
  assert.match(b.text, /Contracts record/);
  assert.match(b.text, /412,345/);
  const b2 = await get("/block/23");
  assert.doesNotMatch(b2.text, /Contracts record/);
  const bj = (await get("/api/block/24")).json();
  assert.equal(bj.contracts.receipt_count, 1);
});

test("a node without the contract RPC: pages say Unavailable with the reason, never empty data", async () => {
  const n2 = fakeNode({ contracts: false });
  await new Promise<void>((r) => n2.listen(0, "127.0.0.1", () => r()));
  const d2 = mkdtempSync(join(tmpdir(), "explorer-contracts-off-"));
  const ix2 = new Indexer({ nodeUrl: `http://127.0.0.1:${(n2.address() as any).port}`, solUrl: `http://127.0.0.1:${PORT}`, stateDir: d2, log: () => {} });
  try {
    await ix2.poll().catch(() => {});
    const r = await route(ix2, "GET", "/contracts");
    assert.match(String(r.body), /Unavailable:<\/strong> this node does not serve listContracts: the contracts upgrade is not deployed on it/);
    assert.match(String(r.body), /getParams has no contract fields/);
    const c = await route(ix2, "GET", `/contract/${COUNTER}`);
    assert.match(String(c.body), /Unavailable:<\/strong> this node does not serve getContract/);
    const t = JSON.parse(String((await route(ix2, "GET", `/api/tx/${CALL.tx_hash}`)).body));
    assert.equal(t.receipt.ok, false);
    assert.match(t.receipt.reason, /no receipt/);
  } finally {
    ix2.stop();
    n2.close();
    rmSync(d2, { recursive: true, force: true });
  }
});
