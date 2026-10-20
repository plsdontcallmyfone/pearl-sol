// HTTP clients: the agent-chain node (JSON-RPC, docs/RPC.md), the inference gateway (SPEC 5.4,
// wallet-signed cadre-auth-v1), cadre-api (town reads), Solana RPC (a recent blockhash for the
// x402 payment). Every call has a timeout; errors carry the server's message.
import { createHash } from "node:crypto";
import type { Wallet } from "./wallet.ts";

async function fetchJson(url: string, init: RequestInit, timeoutMs: number): Promise<{ status: number; body: any; headers: Headers }> {
  const r = await fetch(url, { ...init, signal: AbortSignal.timeout(timeoutMs) });
  const t = await r.text();
  let body: any = t;
  try { body = JSON.parse(t); } catch { /* text */ }
  return { status: r.status, body, headers: r.headers };
}

export class RpcError extends Error {
  code: number;
  constructor(code: number, message: string) { super(message); this.code = code; }
}

export class NodeRpc {
  url: string;
  constructor(url: string) { this.url = url; }
  async call<T = any>(method: string, params: Record<string, unknown> = {}): Promise<T> {
    const r = await fetchJson(this.url, { method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify({ jsonrpc: "2.0", id: 1, method, params }) }, 30_000);
    if (r.body?.error) throw new RpcError(r.body.error.code ?? -1, `${method}: ${r.body.error.message ?? JSON.stringify(r.body.error)}`);
    if (r.status !== 200) throw new Error(`${method}: HTTP ${r.status}`);
    return r.body.result as T;
  }
  sendTx(tx: Uint8Array) { return this.call<{ tx_hash: string }>("sendTx", { tx: Buffer.from(tx).toString("base64") }); }
  putBlob(data: Uint8Array, push = true) { return this.call<{ blob_id: string; uri: string; size: number }>("putBlob", { data: Buffer.from(data).toString("base64"), push }); }
}

export const authMessage = (method: string, path: string, ts: number, body: string) =>
  Buffer.from(`cadre-auth-v1\n${method.toUpperCase()}\n${path.split("?")[0]}\n${ts}\n${createHash("sha256").update(body).digest("hex")}`, "utf8");

export class Gateway {
  url: string;
  wallet: Wallet;
  constructor(url: string, wallet: Wallet) { this.url = url.replace(/\/$/, ""); this.wallet = wallet; }
  private headers(method: string, path: string, body: string): Record<string, string> {
    const ts = Date.now();
    const sig = this.wallet.sign(authMessage(method, path, ts, body));
    return { "content-type": "application/json", "x-cadre-wallet": this.wallet.address, "x-cadre-timestamp": String(ts), "x-cadre-signature": Buffer.from(sig).toString("hex") };
  }
  async chat(model: string | undefined, messages: { role: string; content: string }[], maxTokens: number) {
    const body = JSON.stringify({ ...(model ? { model } : {}), messages, max_tokens: maxTokens });
    return fetchJson(this.url + "/v1/chat/completions", { method: "POST", headers: this.headers("POST", "/v1/chat/completions", body), body }, 900_000);
  }
  async balance() {
    return fetchJson(this.url + "/v1/balance", { method: "GET", headers: this.headers("GET", "/v1/balance", "") }, 30_000);
  }
  async topup(body: Record<string, unknown>, xPayment?: string) {
    const b = JSON.stringify(body);
    return fetchJson(this.url + "/v1/balance/topup", { method: "POST", headers: { "content-type": "application/json", ...(xPayment ? { "x-payment": xPayment } : {}) }, body: b }, 300_000);
  }
  async models() { return fetchJson(this.url + "/v1/models", { method: "GET" }, 30_000); }
}

export class CadreApi {
  url: string;
  constructor(url: string) { this.url = url.replace(/\/$/, ""); }
  async get(path: string) { return fetchJson(this.url + path, { method: "GET" }, 30_000); }
}

export async function latestBlockhash(solRpc: string): Promise<string> {
  const r = await fetchJson(solRpc, { method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify({ jsonrpc: "2.0", id: 1, method: "getLatestBlockhash", params: [{ commitment: "confirmed" }] }) }, 30_000);
  const h = r.body?.result?.value?.blockhash;
  if (!h) throw new Error(`getLatestBlockhash failed: ${JSON.stringify(r.body?.error ?? r.body).slice(0, 200)}`);
  return h;
}
