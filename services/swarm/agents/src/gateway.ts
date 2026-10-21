// The swarm's Gateway client (services/swarm/src/clients.ts) pointed either at the inference
// gateway itself (house setup on box 0) or at cadre-api's pass-through /v1/openai/* (a hosted
// runtime elsewhere, or a self-hosted runner on someone's laptop). The signed path is always the
// gateway's own path (/v1/chat/completions, /v1/balance), as SPEC 5.4 requires, in cadre-auth-v2
// naming the gateway's server key (read once from /v1/models through the same base URL).
import { Gateway } from "../../src/clients.ts";
import type { Wallet } from "../../src/wallet.ts";

async function fetchJson(url: string, init: RequestInit, timeoutMs: number) {
  const r = await fetch(url, { ...init, signal: AbortSignal.timeout(timeoutMs) });
  const t = await r.text();
  let body: any = t;
  try { body = JSON.parse(t); } catch { /* text */ }
  return { status: r.status, body, headers: r.headers };
}

export class GatewayAt extends Gateway {
  base: string;
  viaApi: boolean;
  constructor(url: string, wallet: Wallet) {
    super(url, wallet);
    this.base = url.replace(/\/$/, "");
    this.viaApi = /\/v1\/openai$/.test(this.base);
  }
  /** Public URL of a gateway path (/v1/x -> base/v1/x, or base/x through cadre-api). */
  at(path: string) {
    return this.viaApi ? this.base + path.replace(/^\/v1/, "") : this.base + path;
  }
  override async chat(model: string | undefined, messages: { role: string; content: string }[], maxTokens: number) {
    const body = JSON.stringify({ ...(model ? { model } : {}), messages, max_tokens: maxTokens });
    return fetchJson(this.at("/v1/chat/completions"), { method: "POST", headers: await this.signedHeaders("POST", "/v1/chat/completions", body), body }, 900_000);
  }
  override async balance() {
    return fetchJson(this.at("/v1/balance"), { method: "GET", headers: await this.signedHeaders("GET", "/v1/balance", "") }, 30_000);
  }
  override async topup(body: Record<string, unknown>, xPayment?: string) {
    const b = JSON.stringify(body);
    return fetchJson(this.at("/v1/balance/topup"), { method: "POST", headers: { "content-type": "application/json", ...(xPayment ? { "x-payment": xPayment } : {}) }, body: b }, 300_000);
  }
  override async models() {
    return fetchJson(this.at("/v1/models"), { method: "GET" }, 30_000);
  }
}
