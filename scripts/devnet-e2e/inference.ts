// One real paid inference flow on the public Cadre devnet, from this machine, through the public
// cadre-api only (docs/DEVNET.md "Inference"): a throwaway buyer funded by ops tops up a prepaid
// balance over x402 (cadre-api builds the payment tx, the buyer signs), asks one wallet-signed chat
// completion (receipt headers), closes the balance, then waits for the ticket to complete on the
// chain (every segment mined) and for its storefront Order to be finalized on Solana by
// finalize_order_proof (cadre-settle). Run: scripts/devnet-inference.sh. Devnet only; ops pays.
//   INF_ROWS (default 256), INF_PROMPT, INF_MAX_TOKENS (default 8), INF_FUND_LAMPORTS (default 0.2 SOL)
import { createHash } from "node:crypto";
import { existsSync, mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { Connection, Keypair, PublicKey, SystemProgram, Transaction, VersionedTransaction } from "@solana/web3.js";
import { ORDER_LAYOUT, ORDER_STATUS_FINALIZED, STOREFRONT_PROGRAM_ID, orderPda } from "../../services/relayer/src/codec.ts";

const ROOT = resolve(dirname(fileURLToPath(import.meta.url)), "../..");
const env = process.env;
const DASH = env.BOX0_IP!.replaceAll(".", "-");
const API = env.INF_API ?? `https://api.${DASH}.sslip.io`;
const NODE_RPC = `https://rpc.${DASH}.sslip.io`;
const conn = new Connection(env.SOLANA_RPC!, "confirmed");
const OUT = resolve(ROOT, ".devnet-e2e");
mkdirSync(resolve(OUT, "keys"), { recursive: true });
const RUN = env.INF_RUN ?? new Date().toISOString().replace(/[:.]/g, "-");
const loadKp = (f: string) => Keypair.fromSecretKey(Uint8Array.from(JSON.parse(readFileSync(f, "utf8"))));
const ops = loadKp(resolve(ROOT, "keys/devnet/ops-keypair.json"));
const buyerFile = resolve(OUT, "keys", "inference-buyer.json");
if (!existsSync(buyerFile)) writeFileSync(buyerFile, JSON.stringify(Array.from(Keypair.generate().secretKey)), { mode: 0o600 });
const buyer = loadKp(buyerFile);
const MIN = 60_000;
const t0 = Date.now();
const result: Record<string, unknown> = { run: RUN, api: API, buyer: buyer.publicKey.toBase58() };
const log = (m: string, x?: unknown) => console.log(`[${new Date().toISOString().slice(11, 19)}] ${m}${x === undefined ? "" : " " + JSON.stringify(x)}`);
const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms));
const save = () => writeFileSync(resolve(OUT, `inference-${RUN}.json`), JSON.stringify(result, null, 2));

async function waitFor<T>(desc: string, f: () => Promise<T | null | undefined | false>, timeoutMs: number, everyMs = 15_000): Promise<T> {
  const until = Date.now() + timeoutMs;
  let last = "";
  for (;;) {
    try { const v = await f(); if (v) return v as T; } catch (e) { last = String((e as Error).message ?? e).slice(0, 200); }
    if (Date.now() > until) throw new Error(`timed out waiting for ${desc}${last ? ` (${last})` : ""}`);
    await sleep(everyMs);
  }
}
async function api(path: string, init: RequestInit = {}): Promise<{ status: number; body: any; headers: Headers }> {
  const r = await fetch(API + path, { ...init, signal: AbortSignal.timeout(1_800_000) });
  const t = await r.text();
  let body: any = t;
  try { body = JSON.parse(t); } catch { /* text */ }
  return { status: r.status, body, headers: r.headers };
}
async function nodeCall(method: string, params: Record<string, unknown> = {}): Promise<any> {
  const r = await fetch(NODE_RPC, { method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify({ jsonrpc: "2.0", id: 1, method, params }) });
  const j: any = await r.json();
  if (j.error) throw new Error(`${method}: ${JSON.stringify(j.error)}`);
  return j.result;
}
const { ed25519 } = await import("@noble/curves/ed25519");
let audience = "";
function signed(method: string, gatewayPath: string, body: string): Record<string, string> {
  const ts = Date.now();
  // cadre-auth-v2: names the gateway's server key (from /v1/openai/models), security pass V-9
  const m = `cadre-auth-v2\n${audience}\n${method}\n${gatewayPath}\n${ts}\n${createHash("sha256").update(body).digest("hex")}`;
  const sig = Buffer.from(ed25519.sign(Buffer.from(m), buyer.secretKey.subarray(0, 32))).toString("hex");
  return { "content-type": "application/json", "x-cadre-wallet": buyer.publicKey.toBase58(), "x-cadre-timestamp": String(ts), "x-cadre-signature": sig };
}

// 0. what the API serves
const models = await api("/v1/inference/models");
log("models", { status: models.status, served_by_gateway: models.body?.served_by_gateway ?? models.body?.gateway, ids: (models.body?.data ?? models.body?.models ?? []).map((m: any) => m.id ?? m.name) });
const oaModels = await api("/v1/openai/models");
if (oaModels.status !== 200) throw new Error(`/v1/openai/models ${oaModels.status} ${JSON.stringify(oaModels.body).slice(0, 300)}`);
const model = oaModels.body.data[0];
audience = model.cadre?.server ?? "";
if (!audience) throw new Error("/v1/openai/models names no cadre.server (the cadre-auth-v2 audience)");
result.model = { id: model.id, pricing: model.cadre?.pricing, model_id: model.cadre?.model_id };
log("model", result.model);

// 1. fund the throwaway buyer from ops (devnet SOL)
const fund = BigInt(env.INF_FUND_LAMPORTS ?? 200_000_000);
if (BigInt(await conn.getBalance(buyer.publicKey)) < fund / 2n) {
  const tx = new Transaction().add(SystemProgram.transfer({ fromPubkey: ops.publicKey, toPubkey: buyer.publicKey, lamports: fund }));
  const sig = await conn.sendTransaction(tx, [ops]);
  await conn.confirmTransaction(sig, "confirmed");
  log("buyer funded", { lamports: fund.toString(), sig });
}
const solBefore = await conn.getBalance(buyer.publicKey);

// 2. x402 top-up of a prepaid balance through cadre-api
const rows = Number(env.INF_ROWS ?? 256);
const topBody = JSON.stringify({ rows });
const q = await api("/v1/inference/balance/topup", { method: "POST", headers: { "content-type": "application/json" }, body: topBody });
if (q.status !== 402) throw new Error(`top-up without payment: expected 402, got ${q.status} ${JSON.stringify(q.body).slice(0, 400)}`);
const reqs = q.body;
const acc = reqs.accepts[0];
log("402 requirements", { price_lamports: acc.maxAmountRequired, payTo: acc.payTo, network: acc.network, rows });
const built = await api("/v1/inference/x402/payment-tx", { method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify({ payer: buyer.publicKey.toBase58(), requirements: reqs }) });
if (built.status !== 200) throw new Error(`payment-tx ${built.status} ${JSON.stringify(built.body).slice(0, 400)}`);
const vtx = VersionedTransaction.deserialize(Buffer.from(built.body.transaction, "base64"));
vtx.sign([buyer]);
const xPayment = Buffer.from(JSON.stringify({ x402Version: 1, scheme: "exact", network: built.body.network, payload: { transaction: Buffer.from(vtx.serialize()).toString("base64") } })).toString("base64");
const tPay = Date.now();
const top = await api("/v1/inference/balance/topup", { method: "POST", headers: { "content-type": "application/json", "x-payment": xPayment }, body: topBody });
if (top.status !== 201) throw new Error(`top-up: ${top.status} ${JSON.stringify(top.body).slice(0, 800)}`);
const ticket: string = top.body.ticket_id;
const nonce = BigInt(top.body.order);
result.topup = { ticket, order: top.body.order, rows: top.body.rows, lamports_paid: top.body.lamports_paid, price_lamports: acc.maxAmountRequired, explorer: top.body.explorer_url, wall_s: (Date.now() - tPay) / 1000 };
log("prepaid balance", result.topup);
save();

// 3. the gateway serves the balance at once (its ledger); the chain ticket follows the relayer's unlock

// 4. one wallet-signed chat completion with a receipt
const prompt = env.INF_PROMPT ?? "What is the capital of France? Answer in one word.";
const maxTokens = Number(env.INF_MAX_TOKENS ?? 8);
const chatBody = JSON.stringify({ model: model.id, messages: [{ role: "user", content: prompt }], max_tokens: maxTokens });
const t1 = Date.now();
const r1 = await api("/v1/openai/chat/completions", { method: "POST", headers: signed("POST", "/v1/chat/completions", chatBody), body: chatBody });
if (r1.status !== 200) throw new Error(`chat completion: ${r1.status} ${JSON.stringify(r1.body).slice(0, 800)}`);
const receiptHeaders: Record<string, string | null> = {};
for (const h of ["x-cadre-request", "x-cadre-ticket", "x-cadre-order", "x-cadre-rows", "x-cadre-charged-lamports", "x-cadre-receipt", "x-cadre-explorer"]) receiptHeaders[h] = r1.headers.get(h);
result.chat = { prompt, answer: r1.body.choices[0].message.content, usage: r1.body.usage, measured: r1.body.cadre?.measured, receipt: r1.body.cadre, receipt_headers: receiptHeaders, wall_s: (Date.now() - t1) / 1000 };
log("chat completion", result.chat);
if (receiptHeaders["x-cadre-ticket"] !== ticket) throw new Error(`receipt ticket ${receiptHeaders["x-cadre-ticket"]} != ${ticket}`);
save();

// 5. close the balance: the gateway posts the round's segments, delivers, gets it attested, completes
const rc = await api("/v1/openai/balance/close", { method: "POST", headers: signed("POST", "/v1/balance/close", "{}"), body: "{}" });
log("close", { status: rc.status, body: JSON.stringify(rc.body).slice(0, 300) });
if (rc.status !== 202 && rc.status !== 200) throw new Error(`close: ${rc.status}`);
const tClose = Date.now();
let lastLog = 0;
const done = await waitFor("ticket completed with every segment mined", async () => {
  const t = await nodeCall("getTicket", { ticket_id: ticket });
  const segs = (t?.segment_list ?? []) as any[];
  const mined = segs.filter((g) => g.blocks_mined >= 1).length;
  if (Date.now() - lastLog > 10 * MIN) { lastLog = Date.now(); log("ticket", { status: t?.status, rounds: t?.standing?.rounds, segments: segs.length, mined }); }
  return t && t.status === "completed" && segs.length > 0 && mined === segs.length ? t : null;
}, 8 * 60 * MIN, 30_000);
result.ticket = { status: done.status, segments: done.segment_list.length, rows_used: done.standing?.rows_used, closed_height: done.closed_height, deliverer: done.deliverer, completed_after_close_min: (Date.now() - tClose) / MIN };
log("ticket completed", result.ticket);
save();

// 6. the Order finalized on Solana by proof
const pda = orderPda(nonce);
const a = await waitFor("Order finalized on Solana", async () => {
  const x = await conn.getAccountInfo(pda);
  return x && x.data[ORDER_LAYOUT.STATUS] === ORDER_STATUS_FINALIZED ? x : null;
}, 4 * 60 * MIN, 30_000);
const rh = Buffer.from(a.data.subarray(ORDER_LAYOUT.RESULT_HASH, ORDER_LAYOUT.RESULT_HASH + 32)).toString("hex");
// the finalizing tx is not always the newest one touching the Order (later share settlements touch it too)
const disc = (n: string) => createHash("sha256").update(`global:${n}`).digest().subarray(0, 8);
let how = "unknown";
let sig = { signature: "" };
for (const s of await conn.getSignaturesForAddress(pda, { limit: 20 })) {
  const tx = await conn.getTransaction(s.signature, { commitment: "confirmed", maxSupportedTransactionVersion: 0 });
  for (const ci of tx?.transaction.message.compiledInstructions ?? []) {
    if (!tx!.transaction.message.staticAccountKeys[ci.programIdIndex].equals(STOREFRONT_PROGRAM_ID)) continue;
    for (const n of ["finalize_order_proof", "finalize_order"]) if (Buffer.from(ci.data).subarray(0, 8).equals(disc(n))) { how = n; sig = s; }
  }
  if (how !== "unknown") break;
}
result.order = { pda: pda.toBase58(), finalized_by: how, tx: sig.signature, result_hash: rh, matches_ticket: rh === done.result_hash, settled_after_payment_min: (Date.now() - tPay) / MIN };
result.buyer_sol_spent = (solBefore - (await conn.getBalance(buyer.publicKey))) / 1e9;
result.total_min = (Date.now() - t0) / MIN;
log("Order finalized", result.order);
save();
if (how !== "finalize_order_proof" || rh !== done.result_hash) throw new Error("order not finalized by proof with the ticket's result hash");
console.log("PASS");
