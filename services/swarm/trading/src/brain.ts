// The slow layer: a paid Cadre thought sets the strategy (docs/TRADING.md 4). Streams the
// completion (SSE) so time-to-first-token and total time are measured per thought, signs with
// cadre-auth-v2 through the swarm's gateway client (direct gateway or cadre-api's /v1/openai
// pass-through), tops up its own prepaid balance over x402 with its own SOL when it runs low.
// Inputs to the prompt are numbers this process computed from chain data only; no text written
// by other accounts ever enters a trading prompt (prompt-injection rule, TRADING.md 7).
import { createHash } from "node:crypto";
import { latestBlockhash } from "../../src/clients.ts";
import { paymentHeader, type Requirement } from "../../src/pay.ts";
import type { Wallet } from "../../src/wallet.ts";
import { GatewayAt } from "../../agents/src/gateway.ts";

export interface Thought { text: string; requestId: string; rows: number; ttftMs: number; totalMs: number; textSha256: string; charged: string | null }

export class Brain {
  gw: GatewayAt;
  wallet: Wallet;
  model?: string;
  solRpc: string;
  topupRows: number;
  maxTopupLamportsPerDay: bigint;
  spentToday = 0n;
  day = "";
  log: (e: string, x: Record<string, unknown>) => void;
  constructor(o: { gatewayUrl: string; wallet: Wallet; model?: string; solRpc: string; topupRows?: number; maxTopupLamportsPerDay?: bigint; log?: (e: string, x: Record<string, unknown>) => void }) {
    this.gw = new GatewayAt(o.gatewayUrl, o.wallet);
    this.wallet = o.wallet;
    this.model = o.model;
    this.solRpc = o.solRpc;
    this.topupRows = o.topupRows ?? 2_048;
    this.maxTopupLamportsPerDay = o.maxTopupLamportsPerDay ?? 200_000_000n;
    this.log = o.log ?? (() => {});
  }

  async rowsFree(): Promise<number> {
    const b = await this.gw.balance();
    if (b.status !== 200) throw new Error(`balance ${b.status}: ${JSON.stringify(b.body).slice(0, 200)}`);
    return Number(b.body?.rows_free ?? 0);
  }

  /** Top up when fewer than `min` rows are free, inside the daily SOL cap. */
  async ensureRows(min = 256): Promise<void> {
    const free = await this.rowsFree().catch(() => 0);
    if (free >= min) return;
    const today = new Date().toISOString().slice(0, 10);
    if (today !== this.day) { this.day = today; this.spentToday = 0n; }
    const q = await this.gw.topup({ rows: this.topupRows });
    if (q.status !== 402) throw new Error(`top-up quote: expected 402, got ${q.status} ${JSON.stringify(q.body).slice(0, 200)}`);
    const req: Requirement = q.body?.accepts?.[0];
    const price = BigInt(req.maxAmountRequired);
    if (this.spentToday + price > this.maxTopupLamportsPerDay) throw new Error(`top-up refused: ${price} lamports would pass the daily cap`);
    const bh = await latestBlockhash(this.solRpc);
    this.spentToday += price;
    const r = await this.gw.topup({ rows: this.topupRows }, paymentHeader(this.wallet, req, bh));
    if (r.status !== 201) throw new Error(`top-up: ${r.status} ${JSON.stringify(r.body).slice(0, 200)}`);
    this.log("topup", { rows: r.body?.rows, ticket: r.body?.ticket_id, price_lamports: price.toString() });
  }

  /** One streamed, paid completion. */
  async think(messages: { role: string; content: string }[], maxTokens: number): Promise<Thought> {
    const body = JSON.stringify({ ...(this.model ? { model: this.model } : {}), messages, max_tokens: maxTokens, stream: true });
    const headers = await (this.gw as any).signedHeaders("POST", "/v1/chat/completions", body);
    const t0 = performance.now();
    const res = await fetch(this.gw.at("/v1/chat/completions"), { method: "POST", headers, body, signal: AbortSignal.timeout(900_000) });
    if (res.status !== 200) throw new Error(`gateway ${res.status}: ${(await res.text()).slice(0, 300)}`);
    let requestId = res.headers.get("x-cadre-request") ?? "";
    let rows = Number(res.headers.get("x-cadre-rows") ?? 0);
    let charged = res.headers.get("x-cadre-charged-lamports");
    let text = "";
    let ttft = -1;
    const isSse = (res.headers.get("content-type") ?? "").includes("event-stream");
    if (!isSse) {
      const j: any = await res.json();
      ttft = performance.now() - t0;
      text = j?.choices?.[0]?.message?.content ?? "";
      requestId ||= j?.cadre?.request_id ?? "";
      rows ||= Number(j?.cadre?.rows ?? 0);
    } else {
      const dec = new TextDecoder();
      let buf = "";
      for await (const chunk of res.body as any) {
        buf += dec.decode(chunk, { stream: true });
        let i;
        while ((i = buf.indexOf("\n")) >= 0) {
          const line = buf.slice(0, i).trim();
          buf = buf.slice(i + 1);
          if (!line.startsWith("data:")) continue;
          const d = line.slice(5).trim();
          if (d === "[DONE]") continue;
          try {
            const j = JSON.parse(d);
            const piece = j?.choices?.[0]?.delta?.content ?? "";
            if (piece && ttft < 0) ttft = performance.now() - t0;
            text += piece;
            requestId ||= j?.cadre?.request_id ?? j?.id ?? "";
            if (j?.cadre?.rows) rows = Number(j.cadre.rows);
            if (j?.cadre?.charged_lamports) charged = String(j.cadre.charged_lamports);
          } catch { /* keep-alive or partial */ }
        }
      }
    }
    const totalMs = performance.now() - t0;
    return { text, requestId, rows, ttftMs: ttft < 0 ? totalMs : ttft, totalMs, textSha256: createHash("sha256").update(text).digest("hex"), charged };
  }
}

/** Strategy the slow layer sets; the fast layer only acts inside it. */
export interface Strategy { stance: "buy" | "sell" | "hold"; conviction: number; setAt: number; requestId: string; text: string }

export function strategyPrompt(persona: string, m: { venue: string; price: number; change1h: number | null; change24h: number | null; positionShare: number; realized: number; unit: string }): { role: string; content: string }[] {
  const pct = (x: number | null) => (x === null ? "unknown" : `${(x * 100).toFixed(2)}%`);
  return [
    { role: "system", content: `You are ${persona}, a cautious trading agent on Cadre. You only see numbers. Answer with exactly one word, BUY, SELL or HOLD, then a conviction number from 1 to 5.` },
    { role: "user", content: `Market ${m.venue}. Price ${m.price.toPrecision(6)} ${m.unit} per token. Change 1h ${pct(m.change1h)}, 24h ${pct(m.change24h)}. Your position is ${(m.positionShare * 100).toFixed(0)}% of your limit. Realized PnL ${m.realized.toFixed(6)} ${m.unit}. Decide.` },
  ];
}

/** Parse "BUY 3" style answers; anything else is HOLD 1 (the safe default). */
export function parseStrategy(text: string): { stance: Strategy["stance"]; conviction: number } {
  const t = text.toUpperCase();
  const m = t.match(/\b(BUY|SELL|HOLD)\b[^0-9]{0,20}([1-5])?/);
  if (!m) return { stance: "hold", conviction: 1 };
  const stance = m[1].toLowerCase() as Strategy["stance"];
  const conviction = Math.min(5, Math.max(1, Number(m[2] ?? 2)));
  return { stance, conviction };
}
