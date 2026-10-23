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
  /** Streamed (SSE) so the response headers arrive at once: a long thought on a CPU worker (measured on
   * devnet box 0: 69 s to first token, about 1 token/s) outlives fetch's 300 s headers timeout when the
   * gateway answers only at the end. The chunks are folded back into the non-stream body shape. */
  override async chat(model: string | undefined, messages: { role: string; content: string }[], maxTokens: number) {
    const body = JSON.stringify({ ...(model ? { model } : {}), messages, max_tokens: maxTokens, stream: true });
    const r = await fetch(this.at("/v1/chat/completions"), { method: "POST", headers: { ...(await this.signedHeaders("POST", "/v1/chat/completions", body)), accept: "text/event-stream" }, body, signal: AbortSignal.timeout(1_800_000) });
    if (!(r.headers.get("content-type") ?? "").includes("event-stream")) {
      const t = await r.text();
      let j: any = t;
      try { j = JSON.parse(t); } catch { /* text */ }
      return { status: r.status, body: j, headers: r.headers };
    }
    let text = "", last: any = null, err: any = null, buf = "";
    const dec = new TextDecoder();
    for await (const part of r.body as any) {
      buf += dec.decode(part, { stream: true });
      let i;
      while ((i = buf.indexOf("\n\n")) >= 0) {
        const ev = buf.slice(0, i); buf = buf.slice(i + 2);
        const line = ev.split("\n").find((l) => l.startsWith("data: "));
        if (!line || line === "data: [DONE]") continue;
        let j: any;
        try { j = JSON.parse(line.slice(6)); } catch { continue; }
        if (j.error) err = j.error;
        const d = j.choices?.[0]?.delta?.content;
        if (typeof d === "string") text += d;
        if (j.cadre || j.usage) last = j;
      }
    }
    if (err) return { status: 502, body: { error: err }, headers: r.headers };
    return { status: 200, body: { choices: [{ index: 0, message: { role: "assistant", content: text }, finish_reason: last?.choices?.[0]?.finish_reason ?? null }], usage: last?.usage ?? null, cadre: last?.cadre ?? null }, headers: r.headers };
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
