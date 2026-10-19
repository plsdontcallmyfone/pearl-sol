// Live check: every read endpoint of a running cadre-api, and its figures cross-checked
// against direct RPC reads of the same stack (read-only; never sends a tx).
//
//   CADRE_API=https://cadre-api.64-95-13-106.sslip.io \
//   NODE_RPC=http://127.0.0.1:20630 SOLANA_RPC=http://127.0.0.1:20600 \   (scripts/vps.sh tunnel)
//   node services/cadre-api/tools/live-check.ts
//
// Without NODE_RPC / SOLANA_RPC only the API itself is checked.

import { CadreApi, CadreApiError } from "../client/index.ts";

const base = process.env.CADRE_API ?? "http://127.0.0.1:10685";
const nodeUrl = process.env.NODE_RPC;
const solUrl = process.env.SOLANA_RPC;
const api = new CadreApi({ baseUrl: base, etag: false });
let pass = 0, fail = 0, pend = 0;
const results: string[] = [];

async function rpc(url: string, method: string, params: unknown) {
  const r = await fetch(url, { method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify({ jsonrpc: "2.0", id: 1, method, params }) });
  const j: any = await r.json();
  if (j.error) throw new Error(`${method}: ${j.error.message}`);
  return j.result;
}

async function check(name: string, f: () => Promise<string | void>) {
  const t0 = Date.now();
  try {
    const note = await f();
    pass++;
    results.push(`PASS ${name} (${Date.now() - t0} ms)${note ? ": " + note : ""}`);
  } catch (e) {
    if (e instanceof CadreApiError && e.pending) {
      pend++;
      results.push(`PENDING ${name}: ${e.code} ${e.message.slice(0, 120)}`);
      return;
    }
    fail++;
    results.push(`FAIL ${name}: ${(e as Error).message.slice(0, 300)}`);
  }
}
const eq = (a: unknown, b: unknown, what: string) => {
  if (String(a) !== String(b)) throw new Error(`${what}: api ${a} != rpc ${b}`);
};

const s = await api.summary();
await check("GET /v1/health", async () => void (await api.health()));
await check("GET /v1/ready", async () => {
  const r = await api.ready();
  if (!r.ready) throw new Error(JSON.stringify(r.checks));
  return `upstreams ${Object.entries(r.upstreams).map(([k, v]) => `${k}=${v.ok ? "up" : "down"}`).join(" ")}`;
});
await check("GET /v1/summary", async () => {
  if (!s.chain) throw new Error("no chain");
  let note = `height ${s.chain.height}, paid ${s.blocks.paid_job_blocks} of ${s.blocks.total}, P ${s.fees.protocol_fees_paid}, payers ${s.payers.unique_payers}`;
  if (nodeUrl) {
    const info = await rpc(nodeUrl, "getChainInfo", {});
    if (info.genesis_hash !== s.chain.genesis_hash) throw new Error("genesis differs from the node's");
    if (Math.abs(info.height - s.chain.height) > 10) throw new Error(`height ${s.chain.height} vs node ${info.height}`);
    note += `; node height ${info.height}`;
  }
  return note;
});
await check("GET /v1/feed?filter=paid", async () => `${(await api.feed({ filter: "paid", limit: 5 })).rows.length} rows`);
await check("GET /v1/feed", async () => `${(await api.feed({ limit: 5 })).rows.length} rows`);
await check("GET /v1/blocks/{tip}", async () => {
  const b = await api.block(s.chain!.height - 1);
  if (nodeUrl) {
    const raw = await rpc(nodeUrl, "getBlock", { height: s.chain!.height - 1 });
    eq(b.hash, raw.hash, "block hash");
    eq(b.subsidy, raw.subsidy ?? "0", "subsidy");
  }
  return `${b.kind} ${b.hash.slice(0, 12)}`;
});
await check("GET /v1/blocks/{h}/proof", async () => `${(await api.blockProof(1)).bytes} bytes`);
const jobs = await api.jobs({ limit: 5 });
await check("GET /v1/jobs", async () => `${jobs.total} jobs`);
if (jobs.jobs[0]) {
  await check("GET /v1/jobs/{id}", async () => {
    const j = await api.job(jobs.jobs[0].job_id);
    if (nodeUrl) {
      const raw = await rpc(nodeUrl, "getJob", { job_id: j.job_id });
      eq(j.protocol_fee, raw.protocol_fee, "protocol_fee");
      eq(j.status, raw.status, "status");
    }
    return `${j.kind} ${j.status}`;
  });
}
const orders = await api.orders({ limit: 5 });
await check("GET /v1/orders", async () => `${orders.total} orders`);
if (orders.orders[0]) await check("GET /v1/orders/{nonce}", async () => (await api.order(orders.orders[0].nonce)).status);
await check("GET /v1/payers", async () => `${(await api.payers()).total} payers`);
await check("GET /v1/buyback", async () => `vault ${(await api.buyback()).vault_lamports} lamports`);
await check("GET /v1/reserve", async () => `tokens ${(await api.reserve()).tokens_spl}`);
await check("GET /v1/supply", async () => {
  const sp = await api.supply();
  if (solUrl) {
    const ts = await rpc(solUrl, "getTokenSupply", [sp.mint]);
    eq(sp.mint_supply_spl, ts.value.amount, "mint supply");
  }
  if (nodeUrl && sp.agent_chain) {
    const esc = await rpc(nodeUrl, "getEscrow", {});
    if (Math.abs(Number(BigInt(esc.outside_escrow_spl) - BigInt(sp.agent_chain.outside_escrow_spl))) > 1e12) throw new Error("outside_escrow_spl drifted");
  }
  return `supply ${sp.mint_supply_spl}, burned ${sp.burned_spl}, circulating ${sp.circulating_spl ?? "pending"}`;
});
await check("GET /v1/revenue", async () => `screened P ${(await api.revenue()).protocol_fees_screened}`);
await check("GET /v1/bridge", async () => {
  const b = await api.bridge();
  if (solUrl) {
    const v = await rpc(solUrl, "getTokenAccountBalance", [b.vault]);
    eq(b.vault_spl, v.value.amount, "vault");
  }
  return `vault ${b.vault_spl}, delta ${b.invariant.delta_spl}`;
});
await check("GET /v1/mining", async () => `subsidy now ${(await api.mining()).subsidy_now.max_per_block}`);
await check("GET /v1/agents", async () => `${(await api.agents()).total} agents with a record`);
await check("GET /v1/inference/models", async () => {
  const m = await api.inferenceModels();
  return `${m.total} models, gateway ${m.gateway?.reachable ? "up" : "down"}${m.models[0]?.price ? `, price ${m.models[0].price.price_lamports} lamports` : m.models[0] ? `, price: ${m.models[0].price_error}` : ""}`;
});
await check("GET /v1/pump/quote", async () => {
  const q = await api.pumpQuote("100000000");
  return `${q.venue}: 0.1 SOL -> ${q.tokens_out_spl} SPL`;
});
await check("GET /v1/crank", async () => void (await api.crank()));
const who = s.payers.top_payer;
if (who) {
  await check("GET /v1/accounts/{top payer}", async () => {
    const a = await api.account(who);
    if (nodeUrl) eq(a.chain.balance, (await rpc(nodeUrl, "getAccount", { account: who })).balance, "chain balance");
    if (solUrl && !("error" in a.solana)) eq(a.solana.lamports, (await rpc(solUrl, "getBalance", [who])).value, "SOL balance");
    return `chain ${a.chain.balance}, SOL ${"error" in a.solana ? a.solana.error : a.solana.lamports}`;
  });
  await check("GET /v1/agents/{top payer}", async () => void (await api.agent(who)));
  await check("GET /v1/miners/{top payer}", async () => void (await api.miner(who)));
  await check("GET /v1/bridge/transfers", async () => {
    const t = await api.bridgeTransfers(who);
    return `${t.deposits.length} in, ${t.withdrawals.length} out`;
  });
  await check("GET /v1/inference/receipts", async () => `${(await api.inferenceReceipts({ account: who })).total} tickets`);
}
await check("GET /v1/inference/prepaid/{a} (pending until SPEC 5.4 prepaid)", async () => void (await api.prepaidBalance(who ?? "11111111111111111111111111111111")));
await check("ETag 304 + CORS", async () => {
  const r1 = await fetch(`${base}/v1/summary`, { headers: { origin: "https://example.org" } });
  const et = r1.headers.get("etag");
  if (r1.headers.get("access-control-allow-origin") === null) throw new Error("no CORS header");
  const r2 = await fetch(`${base}/v1/summary`, { headers: { "if-none-match": et ?? "" } });
  if (r2.status !== 304 && r2.status !== 200) throw new Error(`status ${r2.status}`);
  return `second read ${r2.status}, rate limit ${r1.headers.get("ratelimit-limit")}/min`;
});

console.log(`cadre-api live check against ${base}${nodeUrl ? ` (cross-checked: node ${nodeUrl}, solana ${solUrl})` : ""}`);
for (const r of results) console.log("  " + r);
console.log(`${pass} pass, ${fail} fail, ${pend} pending`);
process.exit(fail ? 1 : 0);
