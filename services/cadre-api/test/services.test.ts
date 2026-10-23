// Services market routes (docs/API.md "Services market", docs/MARKET.md) against a fake node
// that serves the market contract's records (encoded with the contract's layout), its events
// and the request / delivery / verdict blobs. Also the reputation rules (providerRecord,
// exclusion) as units. Ports 10752 (fake RPC) and 10753 (api): the Phase 3 row's block.
import { test, before, after } from "node:test";
import assert from "node:assert/strict";
import { createServer, type Server } from "node:http";
import { mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { configFromEnv, Deps } from "../src/deps.ts";
import { Api } from "../src/http.ts";
import { registerServices } from "../src/routes/services.ts";
import { b58decode, b58encode, hex } from "../../../apps/explorer/src/codec.ts";
import { blake3 } from "../../../apps/explorer/src/blake3.ts";
import { W, decodeHire, decodeOffer, eventTopicHex, exclusion, feeOf, marketArgs, providerRecord, type Hire } from "../../../apps/explorer/src/market.ts";

const PORTS = { rpc: 10752, api: 10753 };
const key = (i: number) => b58encode(Uint8Array.from({ length: 32 }, (_, j) => (j === 0 ? i : 9)));
const MARKET = key(1), ADMIN = key(2), PROV = key(3), BUYER = key(4), JUDGE = key(5), HOUSEB = key(6), PROV2 = key(7), POOR = key(9);
const C = 1_000_000_000n;

const blobs = new Map<string, Buffer>();
const putBlob = (json: unknown) => { const b = Buffer.from(JSON.stringify(json)); const id = hex(blake3(b)); blobs.set(id, b); return { uri: `blob:${id}`, id }; };
const SPEC = putBlob({ format: "cadre-offer-v1", description: "four lines on a theme", deliverable: "four lines", samples: ["a", "b"] });
const REQ = putBlob({ format: "cadre-request-v1", request: "the sea at night" });
const DEL_TEXT = "line one\nline two\nline three\nline four";
const DEL = putBlob({ format: "cadre-delivery-v1", text: DEL_TEXT, thought_ref: "req-9" });
const VERD = putBlob({ format: "cadre-verdict-v1", accept: true, reason: "four lines, on topic", answer: "VERDICT: ACCEPT\nREASON: four lines, on topic", limits: "an opinion, not a proof" });

function offerRec(provider: string, title: string, cat: string, price: bigint, completed: number) {
  const w = new W().addr(provider).str(title).str(cat).str(SPEC.uri).hash(SPEC.id).u64(price).u64(20).u8(0).evals([]).bool(true).u64(100).u64(100).u64(completed + 1).u64(completed);
  return w.hex();
}
function hireRec(o: { offer: number; buyer: string; provider: string; price: bigint; status: number; acc: number; delivered: number }) {
  const fee = feeOf(o.price, 2000);
  const w = new W().u64(o.offer).addr(o.buyer).addr(o.provider).u64(o.price).u64(fee).u64(C / 2n).u64(110).u64(170)
    .str(REQ.uri).hash(REQ.id).evals([JUDGE]).u8(1).u8(o.status).u8(o.acc).u8(o.status === 3 ? 1 : 0).u8(o.status >= 2 && o.status <= 3 ? 1 : 0).u8(o.acc)
    .hash(o.delivered ? DEL.id : "00".repeat(32)).str(o.delivered ? DEL.uri : "").u64(o.delivered).u64(o.status >= 2 ? 130 : 0);
  return w.hex();
}
const OFFERS = [offerRec(PROV, "Four-line verse", "creative", 2n * C, 2), offerRec(PROV2, "Name ideas", "naming", 3n * C, 0)];
const HIRES = [
  hireRec({ offer: 0, buyer: BUYER, provider: PROV, price: 2n * C, status: 2, acc: 1, delivered: 120 }), // organic, completed
  hireRec({ offer: 0, buyer: HOUSEB, provider: PROV, price: 2n * C, status: 2, acc: 1, delivered: 121 }), // house buyer
  hireRec({ offer: 1, buyer: BUYER, provider: PROV2, price: 3n * C, status: 0, acc: 0, delivered: 0 }), // open
];
const idT = (i: number) => { const b = Buffer.alloc(32); b.writeBigUInt64LE(BigInt(i)); return b.toString("hex"); };
const aT = (a: string) => Buffer.from(b58decode(a)).toString("hex");
const EVENTS = [ // newest first, like the node
  { height: 130, tx_hash: "cc".repeat(32), topics: [eventTopicHex("Verdict"), idT(0), aT(JUDGE), aT(PROV)], data: new W().bool(true).hash(VERD.id).str(VERD.uri).hash(DEL.id).str("req-judge").hex() },
  { height: 120, tx_hash: "bb".repeat(32), topics: [eventTopicHex("Delivered"), idT(0), aT(PROV), aT(BUYER)], data: new W().hash(DEL.id).str(DEL.uri).str("req-9").hex() },
  { height: 110, tx_hash: "aa".repeat(32), topics: [eventTopicHex("Hired"), idT(0), aT(BUYER), aT(PROV)], data: new W().u64(0).u64(2n * C).u64(feeOf(2n * C, 2000)).u64(C / 2n).u64(170).u8(1).evals([JUDGE]).hash(REQ.id).str(REQ.uri).hex() },
];
let simulateOk = true;
function rpc(m: string, p: any): any {
  switch (m) {
    case "getChainInfo": return { chain_id: 7, genesis_hash: "ab".repeat(32), height: 140, finalized_height: 100, finality_k: 30 };
    case "getParams": return { min_fee: "1000", contract_base_fee: "1000", contract_byte_fee: "10", gas_price: "10000", gas_price_unit: "1000000", tx_gas_limit: "50000000" };
    case "getAccount": return { balance: p.account === POOR ? "0" : (100n * C).toString(), nonce: 3 };
    case "getBlock": return null;
    case "listJobs": return { total: 0, jobs: [] };
    case "getBridgeEvents": return { withdrawals: [] };
    case "simulateCall": if (p.from === POOR) throw new Error(`simulateCall: insufficient balance: have 0, need ${p.value}`); return simulateOk ? { ok: true, gas_used: 700_000 } : { ok: false, error: Buffer.from("a provider cannot hire its own offer").toString("hex") };
    case "getBlob": { const id = String(p.blob_id).replace(/^blob:/, ""); const b = blobs.get(id); return b ? { blob_id: id, data: b.toString("base64") } : null; }
    case "getContractEvents": return { total: EVENTS.length, events: EVENTS.slice(p.offset ?? 0, (p.offset ?? 0) + (p.limit ?? 50)) };
    case "queryContract": {
      assert.equal(p.address, MARKET);
      const a = Buffer.from(p.args, "hex");
      const meth = a.subarray(1, 1 + a[0]).toString();
      const arg = a.length >= 1 + a[0] + 8 ? Number(a.readBigUInt64LE(1 + a[0])) : 0;
      if (meth === "config") return { ok: true, return: new W().addr(ADMIN).u16(2000).addr(ADMIN).bool(false).hex() };
      if (meth === "counts") return { ok: true, return: new W().u64(OFFERS.length).u64(HIRES.length).hex() };
      if (meth === "offer") return { ok: true, return: OFFERS[arg] };
      if (meth === "hire") return { ok: true, return: HIRES[arg] };
      if (meth === "quote") { const price = arg === 0 ? 2n * C : 3n * C; const jf = a.readBigUInt64LE(1 + a[0] + 8); const fee = feeOf(price, 2000); return { ok: true, return: new W().u64(price).u64(fee).u64(jf).u64(price + fee + jf).hex() }; }
      return { ok: false, error: "" };
    }
    default: throw new Error(`unknown method ${m}`);
  }
}

let rpcSrv: Server, apiSrv: Server, dir = "", base = "";
const listen = (s: Server, port: number) => new Promise<void>((r, j) => s.once("error", j).listen(port, "127.0.0.1", () => r()));
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
  await listen(rpcSrv, PORTS.rpc);
  dir = mkdtempSync(join(tmpdir(), "cadre-services-test-"));
  const cfg = configFromEnv({ NODE_RPC: `http://127.0.0.1:${PORTS.rpc}`, SOLANA_RPC: `http://127.0.0.1:${PORTS.rpc}`, GATEWAY_URL: "off", STOREFRONT_URL: "off", POOL_URL: "off" }, { stateDir: dir });
  const d = new Deps(cfg);
  d.ix.o.log = () => {};
  const api = new Api({ allowedOrigins: "*", rateLimitPerMin: 0, rateBurst: 1000, trustProxy: false, maxBody: 1 << 20 });
  registerServices(api, d, { market: MARKET, judge: JUDGE, house: { [HOUSEB]: "house_desk", [JUDGE]: "court_clerk" } });
  api.o.log = undefined;
  apiSrv = api.listen(PORTS.api, "127.0.0.1");
  await new Promise((r) => apiSrv.on("listening", r));
  base = `http://127.0.0.1:${PORTS.api}`;
});
after(() => { apiSrv?.close(); rpcSrv?.close(); rmSync(dir, { recursive: true, force: true }); });
const get = async (p: string) => { const r = await fetch(base + p); return { status: r.status, body: await r.json().catch(() => null) }; };
const post = async (p: string, b: unknown) => { const r = await fetch(base + p, { method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify(b) }); return { status: r.status, body: await r.json().catch(() => null) }; };

test("codec round trip: offer and hire records", () => {
  const o = decodeOffer(0, OFFERS[0]);
  assert.equal(o.provider, PROV);
  assert.equal(o.title, "Four-line verse");
  assert.equal(o.price, (2n * C).toString());
  const h = decodeHire(0, HIRES[0]);
  assert.equal(h.status, "completed");
  assert.deepEqual(h.accepted_by, [JUDGE]);
  assert.equal(h.result_hash, DEL.id);
  assert.equal(h.fee, (4n * C / 10n).toString(), "2,000 bps of 2 CADRE");
  const args = marketArgs.hire({ offer: 1, price: 3n * C, request_uri: REQ.uri, request_hash: REQ.id, deadline_blocks: 40, judge_fee: 0n, evaluators: [JUDGE], threshold: 1 });
  assert.equal(Buffer.from(args.subarray(1, 5)).toString(), "hire");
});

test("summary: organic volume excludes the house buyer", async () => {
  const r = await get("/v1/market");
  assert.equal(r.status, 200);
  assert.equal(r.body.config.fee_bps, 2000);
  assert.equal(r.body.volume.organic, (2n * C).toString());
  assert.equal(r.body.volume.organic_distinct_payers, 1);
  assert.equal(r.body.volume.house, (2n * C).toString());
  assert.equal(r.body.by_status.completed, 2);
  assert.equal(r.body.by_status.open, 1);
  assert.equal(r.body.judge.handle, "court_clerk");
});

test("offers list, filters and record", async () => {
  const r = await get("/v1/market/offers");
  assert.equal(r.body.count, 2);
  assert.equal(r.body.offers[0].id, 0, "sorted by organic paid volume");
  assert.equal(r.body.offers[0].record.paid_volume_organic, (2n * C).toString());
  assert.equal(r.body.offers[0].record.house_volume, (2n * C).toString());
  assert.equal(r.body.offers[0].fee, (4n * C / 10n).toString());
  const f = await get("/v1/market/offers?category=naming");
  assert.deepEqual(f.body.offers.map((o: any) => o.id), [1]);
  const one = await get("/v1/market/offers/0");
  assert.equal(one.body.spec.deliverable, "four lines");
  assert.equal(one.body.spec_hash_ok, true);
  assert.equal(one.body.quote.total, (24n * C / 10n).toString());
  assert.equal((await get("/v1/market/offers/9")).status, 404);
});

test("hire detail: request, delivery hash check, verdict reasoning, timeline", async () => {
  const r = await get("/v1/market/hires/0");
  assert.equal(r.status, 200);
  assert.equal(r.body.request.text, "the sea at night");
  assert.equal(r.body.request.hash_ok, true);
  assert.equal(r.body.delivery.text, DEL_TEXT);
  assert.equal(r.body.delivery.hash_ok, true);
  assert.equal(r.body.delivery.thought.thought_ref, "req-9");
  assert.equal(r.body.delivery.thought.verified, false, "no gateway configured: never claimed verified");
  assert.equal(r.body.verdicts.length, 1);
  assert.equal(r.body.verdicts[0].reason, "four lines, on topic");
  assert.equal(r.body.verdicts[0].reason_hash_ok, true);
  assert.equal(r.body.verdicts[0].judge_kind, "llm_judge");
  assert.deepEqual(r.body.timeline.map((e: any) => e.event), ["Hired", "Delivered", "Verdict"]);
  assert.equal(r.body.hire.counts_as_organic, true);
  const h1 = await get("/v1/market/hires/1");
  assert.equal(h1.body.hire.house, true);
  assert.equal(h1.body.hire.counts_as_organic, false);
  const mine = await get(`/v1/market/hires?buyer=${BUYER}`);
  assert.deepEqual(mine.body.hires.map((h: any) => h.id), [2, 0]);
  assert.equal((await get("/v1/market/hires?status=bogus")).status, 400);
});

test("agent page: offers, record, roles", async () => {
  const r = await get(`/v1/market/agents/${PROV}`);
  assert.equal(r.body.offers.length, 1);
  assert.equal(r.body.record.completed, 2);
  assert.equal(r.body.record.distinct_payers, 1);
  assert.equal(r.body.as_provider.length, 2);
  const j = await get(`/v1/market/agents/${JUDGE}`);
  assert.equal(j.body.as_evaluator.length, 3);
});

test("unsigned hire: plain-words review, exact value, refusals", async () => {
  const r = await post("/v1/tx/market", { from: BUYER, action: "hire", offer: 1, request_uri: REQ.uri, request_hash: REQ.id, evaluators: [JUDGE], threshold: 1, judge_fee: (C / 2n).toString(), deadline_blocks: 40 });
  assert.equal(r.status, 200, JSON.stringify(r.body));
  const total = 3n * C + feeOf(3n * C, 2000) + C / 2n;
  assert.equal(r.body.value, total.toString());
  assert.equal(r.body.window_pearl.params.transaction.kind, "Call");
  assert.equal(r.body.window_pearl.params.transaction.contract, MARKET);
  assert.match(r.body.review_text, /You pay 4\.1 CADRE now, held by the market contract/);
  assert.match(r.body.review_text, /0\.6 CADRE is the market fee \(20%\)/);
  assert.match(r.body.review_text, /comes back to you/);
  assert.match(r.body.review_text, /about 40 minutes/);
  // the provider cannot judge, nor hire itself
  assert.equal((await post("/v1/tx/market", { from: BUYER, action: "hire", offer: 1, request_hash: REQ.id, evaluators: [PROV2] })).status, 400);
  assert.equal((await post("/v1/tx/market", { from: PROV2, action: "hire", offer: 1, request_hash: REQ.id, evaluators: [JUDGE] })).status, 409);
  // price guard
  assert.equal((await post("/v1/tx/market", { from: BUYER, action: "hire", offer: 1, price: "1", request_hash: REQ.id, evaluators: [JUDGE] })).status, 409);
  // the contract's own refusal surfaces as 409 would_fail
  simulateOk = false;
  const f = await post("/v1/tx/market", { from: BUYER, action: "hire", offer: 1, request_hash: REQ.id, evaluators: [JUDGE] });
  simulateOk = true;
  assert.equal(f.status, 409);
  assert.match(JSON.stringify(f.body), /would refuse this/);
  // a buyer short of the value hears it in CADRE, before the node's simulateCall throws its raw "insufficient balance"
  const poor = await post("/v1/tx/market", { from: POOR, action: "hire", offer: 1, request_hash: REQ.id, evaluators: [JUDGE] });
  assert.equal(poor.status, 422);
  assert.equal(poor.body.error.code, "insufficient_funds");
  assert.match(poor.body.error.message, /balance 0 CADRE < .* CADRE needed/);
  // verdict only by a named evaluator on a delivered hire
  assert.equal((await post("/v1/tx/market", { from: BUYER, action: "verdict", hire: 0, accept: true })).status, 400);
  assert.equal((await post("/v1/tx/market", { from: BUYER, action: "nope" })).status, 400);
});

test("reputation rules: self-dealing and house are excluded, money from distinct payers", () => {
  const h = (id: number, buyer: string, price: bigint, status: Hire["status"], delivered: number | null, accepted_by: string[] = [JUDGE]): Hire => ({
    id, offer: 0, buyer, provider: PROV, price: price.toString(), fee: "0", judge_fee: "0", total: price.toString(), created: 100, deadline: 200, request_uri: "", request_hash: "",
    evaluators: [JUDGE], threshold: 1, status, accepts: 1, rejects: 0, voted: [JUDGE], accepted_by, result_hash: null, result_uri: null, delivered, closed: 150,
  });
  const ops: Record<string, string> = { [key(30)]: key(40), [PROV]: key(40) };
  const ctx = { isHouse: (a: string) => a === HOUSEB, operatorOf: (a: string) => ops[a] ?? null, transfersTo: (to: string) => (to === key(31) ? [{ from: PROV, height: 90 }] : []) };
  const hires = [
    h(0, BUYER, 5n * C, "completed", 110),
    h(1, key(8), 3n * C, "completed", 112, [key(8)]),
    h(2, key(30), 7n * C, "completed", 111), // shares the provider's operator
    h(3, key(31), 4n * C, "completed", 111), // funded by the provider before hiring (heuristic)
    h(4, HOUSEB, 6n * C, "completed", 111),
    h(5, BUYER, 1n * C, "expired", 120),
    h(6, BUYER, 1n * C, "rejected", 120),
  ];
  assert.equal(exclusion(hires[2], ctx)?.kind, "self_dealt");
  assert.equal(exclusion(hires[3], ctx)?.heuristic, true);
  assert.equal(exclusion(hires[4], ctx)?.kind, "house");
  const r = providerRecord(hires, PROV, ctx);
  assert.equal(r.paid_volume_organic, (8n * C).toString());
  assert.equal(r.distinct_payers, 2);
  assert.equal(r.top_payer_share_bps, 6250);
  assert.equal(r.self_dealt_excluded, (11n * C).toString());
  assert.equal(r.house_volume, (6n * C).toString());
  assert.equal(r.delivered_then_expired, 1);
  assert.equal(r.rejected, 1);
  assert.equal(r.independent_evaluation_share_bps, 8000, "4 of 5 completed hires were judged by someone other than the buyer");
});
