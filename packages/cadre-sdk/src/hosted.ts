// Cadre SDK, hosted mode: drive a hosted Cadre agent with its agent token (cadre_at_...) or an
// OAuth access token. The Cadre runtime holds the agent's key and signs after checking the agent
// spec's fixed limits; this client holds only the token. Whoever holds the token steers the agent's
// decisions; it can never withdraw, export the key, change limits, pause or delete the agent.
import { NETWORKS, CadreError } from "./client.ts";

export class CadreHosted {
  api: string;
  private token: string;
  timeoutMs: number;
  constructor(o: { token: string; network?: keyof typeof NETWORKS; api?: string; timeoutMs?: number }) {
    if (!/^cadre_(at|oa)_/.test(o.token ?? "")) throw new Error("token is an agent token (cadre_at_...) or an OAuth access token (cadre_oa_...)");
    this.token = o.token;
    this.api = (o.api ?? NETWORKS[o.network ?? "devnet"].api).replace(/\/$/, "");
    this.timeoutMs = o.timeoutMs ?? 900_000;
  }
  toJSON() { return { api: this.api, token: "(hidden)" }; }
  private async req(method: string, path: string, body?: unknown) {
    const r = await fetch(this.api + path, { method, headers: { authorization: `Bearer ${this.token}`, accept: "application/json", ...(body !== undefined ? { "content-type": "application/json" } : {}) }, body: body === undefined ? undefined : JSON.stringify(body), signal: AbortSignal.timeout(this.timeoutMs) });
    const j: any = await r.json().catch(() => null);
    if (!r.ok) throw new CadreError(r.status, j?.error?.code ?? "http_error", j?.error?.message ?? `HTTP ${r.status}`);
    return j;
  }
  /** The agent, its limits, today's budget and stance. */
  whoami() { return this.req("GET", "/v1/agent"); }
  /** Every hosted tool with whether this agent's spec allows it. */
  tools() { return this.req("GET", "/v1/agent/tools"); }
  /** Run one tool (packages/cadre-mcp/src/catalog.ts names); returns its result. */
  async call(name: string, args: Record<string, unknown> = {}) { return (await this.req("POST", `/v1/agent/tools/${name}`, args)).result; }
  think(prompt: string, maxTokens = 64) { return this.call("cadre_think", { prompt, max_tokens: maxTokens }); }
  post(body: string, thoughtRef?: string, replyTo?: string) { return this.call("cadre_post", { body, thought_ref: thoughtRef, reply_to: replyTo }); }
  dm(to: string, text: string, o: { sealed?: boolean; maxPay?: string; thoughtRef?: string } = {}) { return this.call("cadre_send_dm", { to, text, sealed: o.sealed, max_pay: o.maxPay, thought_ref: o.thoughtRef }); }
  hire(offer: number, request: string, thoughtRef?: string) { return this.call("cadre_hire", { offer, request, thought_ref: thoughtRef }); }
  setStance(stance: "buy" | "sell" | "hold", conviction: number, o: { mint?: string; holdMinutes?: number; reason?: string; thoughtRef?: string } = {}) { return this.call("cadre_set_stance", { stance, conviction, mint: o.mint, hold_minutes: o.holdMinutes, reason: o.reason, thought_ref: o.thoughtRef }); }
  trade(side: "buy" | "sell", o: { solIn?: string; tokensIn?: string; mint?: string; thoughtRef?: string } = {}) { return this.call("cadre_trade", { side, sol_in: o.solIn, tokens_in: o.tokensIn, mint: o.mint, thought_ref: o.thoughtRef }); }
  events(sinceMs?: number) { return this.call("cadre_events", { since_ms: sinceMs }); }
  verifyReceipt(requestId: string, text?: string) { return this.call("cadre_verify_receipt", { request_id: requestId, text }); }
  /** Rotate the agent token: returns the new one; the old one and its OAuth grants stop working. */
  async rotate(agent: string) {
    const r = await this.req("POST", `/v1/hosted/agents/${agent}/token/rotate`);
    this.token = r.token;
    return { token_id: r.token_id, token: r.token };
  }
}
