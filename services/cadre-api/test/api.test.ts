// cadre-api against the in-process fake stack (test/fake.ts): every route group, the
// HTTP contract (CORS, ETag/304, rate limit, error shape), and that the API never builds
// anything a wallet should not sign.
import { test, before, after } from "node:test";
import assert from "node:assert/strict";
import { mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import type { Server } from "node:http";
import { configFromEnv, Deps } from "../src/deps.ts";
import { build } from "../src/app.ts";
import { EVALUATOR, J1, MINER, MODEL, OWNER, POSTER, SERVER, SF_WALLET, T1, h64, makeState, startFakeStack, type FakeState } from "./fake.ts";
import { parseWireTx } from "../src/sol.ts";
import { useCardProber } from "../src/routes/agents.ts";
import { CardProber, cardConfig } from "../../../apps/explorer/src/agentcard.ts";

let st: FakeState;
let stack: Awaited<ReturnType<typeof startFakeStack>>;
let srv: Server;
let base = "";
let dir = "";
let d: Deps;

async function boot(env: Record<string, string> = {}) {
  const cfg = configFromEnv(
    { NODE_RPC: stack.urls.node, SOLANA_RPC: stack.urls.sol, GATEWAY_URL: stack.urls.gateway, STOREFRONT_URL: stack.urls.storefront, POOL_URL: stack.urls.pool, CADRE_RATE_LIMIT_PER_MIN: "100000", CADRE_RATE_BURST: "100000", ...env },
    { stateDir: dir },
  );
  const deps = new Deps(cfg);
  deps.ix.o.log = () => {};
  await deps.ix.poll();
  const api = build(deps);
  api.o.log = undefined;
  const s = api.listen(0, "127.0.0.1");
  await new Promise((r) => s.on("listening", r));
  return { deps, s, url: `http://127.0.0.1:${(s.address() as any).port}` };
}

before(async () => {
  st = makeState();
  stack = await startFakeStack(st);
  dir = mkdtempSync(join(tmpdir(), "cadre-api-test-"));
  const b = await boot();
  d = b.deps;
  srv = b.s;
  base = b.url;
});
after(() => {
  srv?.close();
  stack?.close();
  rmSync(dir, { recursive: true, force: true });
});

const get = async (p: string, h: Record<string, string> = {}) => {
  const r = await fetch(base + p, { headers: h });
  const text = await r.text();
  let body: any = null;
  try {
    body = JSON.parse(text);
  } catch {
    body = text;
  }
  return { status: r.status, body, headers: r.headers };
};
const post = async (p: string, b: unknown, h: Record<string, string> = {}) => {
  const r = await fetch(base + p, { method: "POST", headers: { "content-type": "application/json", ...h }, body: typeof b === "string" ? b : JSON.stringify(b) });
  const text = await r.text();
  let body: any = null;
  try {
    body = JSON.parse(text);
  } catch {
    body = text;
  }
  return { status: r.status, body, headers: r.headers };
};

test("meta: index, openapi, health, ready", async () => {
  const i = await get("/v1");
  assert.equal(i.status, 200);
  assert.ok(i.body.endpoints.includes("GET /v1/summary"));
  const o = await get("/v1/openapi.yaml");
  assert.equal(o.status, 200);
  assert.match(o.headers.get("content-type")!, /yaml/);
  assert.match(o.body, /^openapi: 3\.1/m);
  assert.equal((await get("/v1/health")).body.ok, true);
  const r = await get("/v1/ready");
  assert.equal(r.status, 200, JSON.stringify(r.body));
  assert.equal(r.body.ready, true);
  assert.equal(r.body.upstreams.gateway.ok, true);
});

test("http contract: error shape, 404/405, CORS, ETag 304, rate limit", async () => {
  const nf = await get("/v1/nope");
  assert.equal(nf.status, 404);
  assert.equal(nf.body.error.code, "not_found");
  assert.match(nf.body.request_id, /^[0-9a-f]{16}$/);
  const ma = await post("/v1/summary", {});
  assert.equal(ma.status, 405);
  assert.equal(ma.headers.get("allow"), "GET");
  const s = await get("/v1/summary", { origin: "https://cadre.example" });
  assert.equal(s.headers.get("access-control-allow-origin"), "*");
  const etag = s.headers.get("etag")!;
  assert.match(etag, /^W\/"/);
  assert.match(s.headers.get("cache-control")!, /max-age=3/);
  const again = await fetch(base + "/v1/summary", { headers: { "if-none-match": etag } });
  assert.equal(again.status, 304);
  const pre = await fetch(base + "/v1/tx/solana", { method: "OPTIONS", headers: { origin: "https://x.example", "access-control-request-method": "POST" } });
  assert.equal(pre.status, 204);
  assert.match(pre.headers.get("access-control-allow-headers")!, /x-payment/);
  const bad = await get("/v1/feed?limit=0");
  assert.equal(bad.status, 400);
  assert.equal(bad.body.error.code, "bad_request");
  // a strict deployment: allow-list origins + a tiny bucket
  const b2 = await boot({ CADRE_ALLOWED_ORIGINS: "https://cadre.example", CADRE_RATE_LIMIT_PER_MIN: "60", CADRE_RATE_BURST: "2" });
  try {
    const r1 = await fetch(b2.url + "/v1/health", { headers: { origin: "https://cadre.example" } });
    assert.equal(r1.headers.get("access-control-allow-origin"), "https://cadre.example");
    const r2 = await fetch(b2.url + "/v1/health", { headers: { origin: "https://evil.example" } });
    assert.equal(r2.headers.get("access-control-allow-origin"), null);
    const r3 = await fetch(b2.url + "/v1/health");
    assert.equal(r3.status, 429);
    assert.ok(Number(r3.headers.get("retry-after")) >= 1);
    assert.equal((await r3.json()).error.code, "rate_limited");
  } finally {
    b2.s.close();
  }
});

test("stats: summary, feed, blocks, jobs, orders, payers, supply, reserve, revenue", async () => {
  const s = await get("/v1/summary");
  assert.equal(s.status, 200);
  assert.equal(s.body.blocks.total, 12);
  assert.equal(s.body.blocks.paid_job_blocks, 2);
  assert.equal(s.body.reserve.tokens_spl, "77000000");
  assert.equal(s.body.supply.mint_supply_spl, "999000000000000");
  assert.equal(s.body.supply.burned_spl, "1000000000000");
  assert.equal(s.body.supply.circulating_spl, null);
  assert.equal(s.body.buyback.vault_lamports, "3000000");
  const f = await get("/v1/feed?filter=paid&limit=5");
  assert.deepEqual(f.body.rows.map((r: any) => r.block.height), [8, 5]);
  assert.equal(f.body.rows[0].block.kind, "paid_inference");
  const b = await get("/v1/blocks/5");
  assert.equal(b.body.job_id, J1);
  assert.equal((await get("/v1/blocks/999")).status, 404);
  assert.equal((await get("/v1/blocks/xyz")).status, 400);
  assert.equal((await get("/v1/blocks/5/proof")).body.bytes, 5);
  const j = await get("/v1/jobs?limit=2");
  assert.equal(j.body.total, 5);
  assert.equal(j.body.jobs.length, 2);
  assert.equal(j.body.next_offset, 2);
  assert.equal((await get("/v1/jobs?kind=ticket")).body.jobs[0].job_id, T1);
  assert.equal((await get(`/v1/jobs/${J1.slice(0, 8)}`)).body.job_id, J1);
  // evaluator index: jobs naming an account among their evaluators
  const ev = await get(`/v1/jobs?evaluator=${EVALUATOR}`);
  assert.equal(ev.status, 200, JSON.stringify(ev.body));
  assert.equal(ev.body.evaluator, EVALUATOR);
  assert.deepEqual(ev.body.jobs.map((x: any) => x.job_id).sort(), [J1, h64("e1")].sort());
  assert.deepEqual(ev.body.counts, { all: 2, open: 0, delivered: 0, completed: 1, expired: 1, rejected: 0 });
  assert.deepEqual(ev.body.jobs.find((x: any) => x.job_id === J1).evaluators, [EVALUATOR, POSTER]);
  assert.deepEqual((await get(`/v1/jobs?evaluator=${EVALUATOR}&status=expired`)).body.jobs.map((x: any) => x.job_id), [h64("e1")]);
  assert.equal((await get(`/v1/jobs?role=evaluator&account=${POSTER}`)).body.total, 1);
  assert.equal((await get(`/v1/jobs?evaluator=${EVALUATOR}&status=bogus`)).status, 400);
  assert.equal((await get("/v1/jobs?evaluator=nope")).status, 400);
  assert.equal((await get("/v1/jobs?role=evaluator")).status, 400);
  assert.equal((await get(`/v1/jobs?evaluator=${MINER}`)).body.total, 0);
  assert.equal((await get("/v1/orders")).body.total, 0);
  assert.equal((await get("/v1/orders/1")).status, 404);
  assert.equal((await get("/v1/payers")).body.payers[0].payer, SERVER);
  const sup = await get("/v1/supply");
  assert.equal(sup.body.bridge_vault_spl, "5000000");
  assert.equal(sup.body.agent_chain.outside_escrow_spl, "5000000");
  const res = await get("/v1/reserve");
  assert.equal(res.body.totals.funded_spl, "100000000");
  assert.equal(res.body.limits.max_slippage_bps, 300);
  const rev = await get("/v1/revenue");
  // self-dealt job f1 (999000) excluded: J1 100000 + T1 200000 + expired e1 50000
  assert.equal(rev.body.protocol_fees_paid, "1349000");
  assert.equal(rev.body.screens.self_dealt_protocol_fees, "999000");
  assert.equal(rev.body.protocol_fees_screened, "350000");
  assert.equal(rev.body.unique_payers_screened, 1);
});

test("agents: card probe on the profile and in the list; credit line from the payout lien", async () => {
  const seen: string[] = [];
  useCardProber(new CardProber(cardConfig(), async (uri: string) => (seen.push(uri), { ok: true as const, card: { name: "server-one card" } as any })));
  const p = await get(`/v1/agents/${SERVER}?probe=1`);
  assert.equal(p.body.registration.name, "server-one");
  assert.equal(p.body.card_probe.ok, true);
  assert.equal(p.body.card_probe.status, "ok");
  assert.equal(p.body.card_probe.card_name, "server-one card");
  assert.match(p.body.card_probe.checked_at, /^\d{4}-/);
  assert.deepEqual(seen, ["https://agent.example/card.json"]);
  const l = await get("/v1/agents?sort=paid&limit=7");
  const row = l.body.agents.find((a: any) => a.account === SERVER);
  assert.equal(row.registration.name, "server-one");
  assert.equal(row.card_probe.ok, true);
  const m = await get(`/v1/accounts/${MINER}`);
  assert.equal(m.body.credit.limit, "1000000000000000");
  assert.equal(m.body.credit.repaid, "1000000500");
  assert.equal(m.body.credit.lender, SF_WALLET);
  assert.equal(m.body.credit.active, true);
  assert.equal(m.body.credit.agent_credit_program.deployed, false);
  assert.equal((await get(`/v1/accounts/${OWNER}`)).body.credit, null);
});

test("agents: profile and list sorted by screened P, never by counts", async () => {
  const p = await get(`/v1/agents/${SERVER}`);
  assert.equal(p.status, 200);
  assert.equal(p.body.registration.name, "server-one");
  assert.equal(p.body.as_server.completed.count, 2);
  assert.equal(p.body.as_server.completed.protocol_fees, "300000");
  assert.equal(p.body.as_server.self_dealt.protocol_fees, "999000");
  assert.equal(p.body.as_server.delivered_then_expired.protocol_fees, "50000");
  assert.equal(p.body.as_server.acceptance_ppm_by_fee, Math.floor((300000 * 1e6) / 350000));
  assert.equal(p.body.as_server.distinct_payers, 1);
  assert.equal(p.body.as_server.models[0].model_id, MODEL);
  const l = await get("/v1/agents?sort=paid");
  assert.equal(l.body.agents[0].account, SERVER);
  assert.equal(l.body.agents[0].paid_protocol_fees, "300000");
  assert.ok(l.body.agents.some((a: any) => a.account === MINER));
  assert.equal((await get("/v1/agents?sort=jobs")).status, 400);
  assert.equal((await get("/v1/agents/notakey")).status, 400);
});

test("mining: params, subsidy rule, miner record with pool", async () => {
  const m = await get("/v1/mining");
  assert.equal(m.body.subsidy_now.max_per_block, "10000000"); // 1e9 x 10000 / 1e6
  assert.equal(m.body.pool.stats.miners, 2);
  const mi = await get(`/v1/miners/${MINER}`);
  assert.deepEqual(mi.body.blocks, { paid_job: 2, fallback: 10, total: 12 });
  assert.equal(mi.body.subsidy, "80000");
  assert.equal(mi.body.pool.shares, 5);
});

test("accounts + bridge: balances, transfers, quotes", async () => {
  const a = await get(`/v1/accounts/${OWNER}`);
  assert.equal(a.status, 200);
  assert.equal(a.body.chain.balance, "5000000000");
  assert.equal(a.body.solana.lamports, "7000000000");
  assert.equal(a.body.solana.spl, "9000000");
  const bal = await get(`/v1/accounts/${OWNER}/balances`);
  assert.equal(bal.body.chain.nonce, 3);
  const br = await get("/v1/bridge");
  assert.equal(br.body.invariant.delta_spl, "0");
  const t = await get(`/v1/bridge/transfers?owner=${OWNER}`);
  assert.equal(t.body.deposits.length, 1);
  assert.equal(t.body.deposits[0].stage, "final");
  assert.equal(t.body.deposits[0].amount_chain, "2000000");
  assert.equal(t.body.withdrawals[0].stage, "final");
  const qi = await get("/v1/bridge/quote?direction=in&amount_spl=2000");
  assert.equal(qi.body.receive_chain, "2000000");
  assert.equal((await get("/v1/bridge/quote?direction=out&amount=1500")).status, 400);
  assert.equal((await get("/v1/bridge/quote?direction=out&amount=2000000")).body.receive_spl, "2000");
});

test("unsigned txs: bridge deposit, bridge lock, x402 payment; broadcast checks signatures", async () => {
  const dep = await post("/v1/tx/bridge-deposit", { owner: OWNER, amount_spl: "2000" });
  assert.equal(dep.status, 200, JSON.stringify(dep.body));
  const tx = parseWireTx(Uint8Array.from(Buffer.from(dep.body.transaction, "base64")));
  assert.deepEqual(tx.signers, [OWNER]);
  assert.ok(tx.sigs[0].every((x) => x === 0), "returned unsigned");
  assert.equal((await post("/v1/tx/bridge-deposit", { owner: OWNER, amount_spl: "999999999" })).status, 422);
  assert.equal((await post("/v1/tx/bridge-deposit", { owner: "bad", amount_spl: "1" })).status, 400);

  const lock = await post("/v1/tx/bridge-lock", { from: OWNER, amount: "2000000" });
  assert.equal(lock.body.window_pearl.params.transaction.kind, "BridgeLock");
  assert.equal(lock.body.window_pearl.params.transaction.solanaRecipient, OWNER);
  assert.equal((await post("/v1/tx/bridge-lock", { from: OWNER, amount: "9000000000000" })).status, 422);

  // x402: the 402 from the gateway pass-through, then the tx to sign
  const q402 = await post("/v1/openai/completions", { prompt: "hi", max_tokens: 4 });
  assert.equal(q402.status, 402);
  const pay = await post("/v1/inference/x402/payment-tx", { payer: OWNER, requirements: q402.body });
  assert.equal(pay.status, 200, JSON.stringify(pay.body));
  const ptx = parseWireTx(Uint8Array.from(Buffer.from(pay.body.transaction, "base64")));
  assert.deepEqual(ptx.signers, [SF_WALLET, OWNER]);
  const evil = { ...q402.body.accepts[0], payTo: OWNER, extra: { ...q402.body.accepts[0].extra, feePayer: OWNER } };
  assert.equal((await post("/v1/inference/x402/payment-tx", { payer: POSTER, requirements: evil })).status, 422);

  // broadcast: unsigned bytes are refused before reaching the RPC
  const un = await post("/v1/tx/solana", { tx: dep.body.transaction });
  assert.equal(un.status, 400);
  assert.deepEqual(un.body.error.details.unsigned, [OWNER]);
  const signed = Buffer.from(dep.body.transaction, "base64");
  signed.fill(7, 1, 65);
  const ok = await post("/v1/tx/solana", { tx: signed.toString("base64") });
  assert.equal(ok.status, 200);
  assert.equal(st.sent.solana.length, 1);
  assert.equal((await get(`/v1/tx/solana/${ok.body.signature}`)).body.status, "finalized");
  const c0 = Buffer.alloc(140, 1);
  const cs = await post("/v1/tx/chain", { tx: c0.toString("base64") });
  assert.equal(cs.status, 200);
  assert.equal(st.sent.chain.length, 1);
  assert.equal((await post("/v1/tx/chain", { tx: Buffer.alloc(140).toString("base64") })).status, 400);
  assert.equal((await get(`/v1/tx/chain/${h64("ab")}`)).body.status, "included");
  assert.equal((await get(`/v1/tx/chain/${h64("00")}`)).status, 404);
});

test("inference: models with prices, quote, receipts, pass-through (402, paid, SSE), prepaid pending", async () => {
  const m = await get("/v1/inference/models?prompt_tokens=6&max_tokens=10");
  assert.equal(m.status, 200, JSON.stringify(m.body));
  assert.equal(m.body.models[0].model_id, MODEL);
  assert.equal(m.body.models[0].served_by_gateway, true, "gateway id is the name, model_id under cadre");
  assert.equal(m.body.models[0].gateway.id, "llama-test");
  assert.equal(m.body.models[0].price.price_lamports, "25111112");
  const q = await get(`/v1/inference/quote?model_id=${MODEL}&prompt_tokens=6&max_tokens=10`);
  assert.equal(q.body.ticket.model_id, MODEL);
  assert.equal((await get(`/v1/inference/quote?model_id=${h64("99")}&prompt_tokens=6&max_tokens=10`)).status, 400);
  const r = await get(`/v1/inference/receipts?account=${POSTER}`);
  assert.equal(r.body.receipts[0].ticket_id, T1);
  assert.equal(r.body.receipts[0].blocks_mined, 1);
  const r1 = await get(`/v1/inference/receipts/${T1}`);
  assert.equal(r1.body.gateway_record.stage, "completed");
  const paid = await post("/v1/openai/completions", { prompt: "hi" }, { "x-payment": "abc" });
  assert.equal(paid.status, 201);
  assert.equal(paid.headers.get("x-payment-response"), "eyJzdWNjZXNzIjp0cnVlfQ==");
  assert.equal(st.gatewayCalls.at(-1)!.headers["x-payment"], "abc");
  const sse = await fetch(base + "/v1/openai/chat/completions", { method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify({ stream: true, messages: [] }) });
  assert.match(sse.headers.get("content-type")!, /event-stream/);
  assert.match(await sse.text(), /data: \[DONE\]/);
  const models = await get("/v1/openai/models");
  assert.equal(models.body.data[0].cadre.model_id, MODEL);
  assert.equal((await get("/v1/openai/admin")).status, 404);
});

test("prepaid through the gateway: balance (public + wallet-signed), top-up 402 -> paid, quote, close, keys, receipt headers, CORS", async () => {
  const pp = await get(`/v1/inference/prepaid/${OWNER}`);
  assert.equal(pp.status, 200, JSON.stringify(pp.body));
  assert.equal(pp.body.rows_free, 200);
  // wallet auth headers are forwarded unchanged; the site's old X-Cadre-Account is sent as X-Cadre-Wallet
  const auth = { "x-cadre-wallet": OWNER, "x-cadre-timestamp": "1", "x-cadre-signature": "ab".repeat(64) };
  const b = await get("/v1/inference/balance", auth);
  assert.equal(b.body.wallet, OWNER);
  assert.equal(st.gatewayCalls.at(-1)!.path, "/v1/balance");
  assert.equal(st.gatewayCalls.at(-1)!.headers["x-cadre-signature"], "ab".repeat(64));
  await get("/v1/inference/balance", { "x-cadre-account": OWNER, "x-cadre-timestamp": "1", "x-cadre-signature": "cd".repeat(64) });
  assert.equal(st.gatewayCalls.at(-1)!.headers["x-cadre-wallet"], OWNER);
  assert.equal((await get(`/v1/inference/balance?wallet=${OWNER}`)).body.wallet, OWNER);
  const t402 = await post("/v1/inference/balance/topup", { rows: 256 });
  assert.equal(t402.status, 402);
  assert.equal(t402.body.accepts[0].maxAmountRequired, "95411375");
  const q = await post("/v1/inference/balance/topup/quote", { rows: 256 }, { "x-payment": "ignored" });
  assert.equal(q.status, 200);
  assert.equal(q.body.price_lamports, "95411375");
  assert.equal(st.gatewayCalls.at(-1)!.headers["x-payment"], undefined);
  const paid = await post("/v1/inference/prepaid/topup", { rows: 256 }, { "x-payment": "pay" });
  assert.equal(paid.status, 201);
  assert.equal(st.gatewayCalls.at(-1)!.path, "/v1/balance/topup");
  assert.equal((await post("/v1/inference/balance/close", {}, auth)).status, 202);
  // keys through the pass-through (POST and DELETE), body byte for byte
  const k = await post("/v1/openai/keys", {}, auth);
  assert.equal(k.status, 201);
  assert.equal(st.gatewayCalls.at(-1)!.body, "{}");
  const del = await fetch(base + "/v1/openai/keys/k1", { method: "DELETE", headers: auth });
  assert.equal(del.status, 200);
  assert.equal(st.gatewayCalls.at(-1)!.method, "DELETE");
  // keys list (GET, wallet-signed): pass-through and the /v1/inference/keys alias, both to gateway /v1/keys
  for (const path of ["/v1/openai/keys", "/v1/inference/keys"]) {
    const kl = await fetch(base + path, { headers: auth });
    assert.equal(kl.status, 200, path);
    const kb = (await kl.json()) as any;
    assert.equal(kb.keys[0].key_id, "k1");
    assert.equal(st.gatewayCalls.at(-1)!.path, "/v1/keys");
    assert.equal(st.gatewayCalls.at(-1)!.method, "GET");
    assert.equal(st.gatewayCalls.at(-1)!.headers["x-cadre-wallet"], auth["x-cadre-wallet"]);
  }
  // receipt headers copied and exposed
  const c = await post("/v1/openai/completions", { prompt: "hi" }, auth);
  assert.equal(c.headers.get("x-cadre-ticket"), T1);
  assert.equal(c.headers.get("x-cadre-explorer"), "http://e/job/" + T1);
  assert.match(c.headers.get("access-control-expose-headers")!, /x-cadre-charged-lamports/);
  const pre = await fetch(base + "/v1/openai/chat/completions", { method: "OPTIONS", headers: { origin: "http://site", "access-control-request-headers": "x-cadre-wallet" } });
  assert.equal(pre.status, 204);
  assert.match(pre.headers.get("access-control-allow-headers")!, /x-cadre-wallet/);
  assert.match(pre.headers.get("access-control-allow-methods")!, /DELETE/);
});

test("prepaid off: 501 pending_upstream", async () => {
  const s2 = await boot({ CADRE_PREPAID: "off" });
  try {
    const r = await fetch(s2.url + `/v1/inference/prepaid/${OWNER}`);
    assert.equal(r.status, 501);
    assert.equal(((await r.json()) as any).error.code, "pending_upstream");
  } finally {
    s2.s.close();
  }
});

test("market: pump quote from the curve, crank view (unsigned txs: test/market-tx.test.ts)", async () => {
  const q = await get("/v1/pump/quote?sol_in=1000000000");
  assert.equal(q.body.venue, "curve");
  // 1e15 x 1e9 / (3e10 + 1e9)
  assert.equal(q.body.tokens_out_spl, ((10n ** 15n * 10n ** 9n) / (30_000_000_000n + 1_000_000_000n)).toString());
  const c = await get("/v1/crank");
  // the fake reserve Config predates the fund limits: fund_mining would fail with NeedsMigration
  assert.equal(c.body.fund_mining.ok, false);
  assert.equal(c.body.fund_mining.code, "needs_migration");
  assert.equal(c.body.fund_mining.next_allowed_ts, null);
  assert.equal((await post("/v1/tx/pump-buy", {})).status, 400);
  // the site sends {payer}: accepted as the cranker (the fake reserve then refuses with needs_migration)
  const fm = await post("/v1/tx/fund-mining", { payer: OWNER });
  assert.notEqual(fm.status, 400, JSON.stringify(fm.body));
  assert.equal((await post("/v1/tx/fund-mining", {})).status, 400);
});

void d;
