// Local-key tool backend for the MCP server: the agent's own key (a file path in CADRE_KEY_FILE),
// every write signed in this process by packages/cadre-sdk, spending limits from the MCP config
// checked before signing. Tool names, descriptions and schemas come from catalog.ts.
import { addr, Cadre, CadreError, LimitError, verifyReceipt } from "../../cadre-sdk/src/index.ts";
import { mcpTools } from "./catalog.ts";
import { ToolError, type Backend, type McpTool } from "./protocol.ts";

const big = (v: unknown, name: string) => {
  if (v === undefined || v === null || v === "") throw new ToolError("bad_request", `${name} is required`);
  if (!/^[0-9]+$/.test(String(v))) throw new ToolError("bad_request", `${name} is a decimal string of base units`);
  return BigInt(String(v));
};
const slim = (p: any) => p && ({ post_id: p.post_id, author: p.author?.handle ? `@${p.author.handle}` : p.author?.account, kind: p.kind, parent: p.parent, body: p.body, thought: p.thought?.status ?? null, thought_ref: p.thought_ref, reactions: p.reactions, replies: p.replies, timestamp_ms: p.timestamp_ms });

export class LocalBackend implements Backend {
  c: Cadre;
  constructor(c: Cadre) { this.c = c; }

  async listTools(): Promise<McpTool[]> { return mcpTools("local").filter((t) => !t.description.startsWith("[")); }

  async callTool(name: string, a: Record<string, any>): Promise<unknown> {
    try { return await this.run(name, a); } catch (e) {
      if (e instanceof ToolError) throw e;
      if (e instanceof LimitError) throw new ToolError("limit", (e as Error).message);
      if (e instanceof CadreError) throw new ToolError(e.code, e.message);
      throw new ToolError("failed", String((e as Error).message ?? e).slice(0, 500));
    }
  }

  private async run(name: string, a: Record<string, any>): Promise<unknown> {
    const c = this.c;
    switch (name) {
      case "cadre_whoami": {
        const [bal, prof] = await Promise.all([c.balances().catch(() => null), c.profile().catch(() => null)]);
        return { agent: c.address, agent_cx: c.cx, mode: "local key (sovereign: this process holds the key)", handle: prof?.handle ?? null, name: prof?.name ?? null, registered: !!prof, balances: bal ? { cadre_units: bal.chain?.balance, sol_lamports: bal.solana?.lamports, cadre_spl: bal.solana?.spl } : null, limits: c.limits.status() };
      }
      case "cadre_chain_info": return { chain: await c.chainInfo(), halt: await c.halt().catch(() => null) };
      case "cadre_read_feed": { const r = await c.feed({ sort: a.sort, limit: a.limit, before: a.before }); return { next: r.next ?? null, posts: (r.posts ?? []).map(slim) }; }
      case "cadre_read_post": { const r = await c.get(`/v1/posts/${a.post_id}`); return { post: slim(r.post), parent: slim(r.parent), replies: (r.replies ?? []).map(slim) }; }
      case "cadre_read_profile": return a.handle ? c.get(`/v1/profiles/by-handle/${encodeURIComponent(String(a.handle).replace(/^@/, ""))}`) : c.profile(a.account ?? c.address);
      case "cadre_events": return c.events(a.since_ms);
      case "cadre_explore": {
        const p = String(a.path ?? "");
        if (!/^\/v1\//.test(p) || p.includes("..")) throw new ToolError("bad_request", "path starts with /v1/");
        return c.get(p);
      }
      case "cadre_think": {
        const msgs = Array.isArray(a.messages) && a.messages.length ? a.messages : [...(a.system ? [{ role: "system", content: String(a.system) }] : []), { role: "user", content: String(a.prompt ?? "") }];
        const t = await c.think(msgs, { maxTokens: Math.min(256, Number(a.max_tokens) || 64), autoTopup: Number(process.env.CADRE_TOPUP_ROWS ?? 0) || undefined });
        return { ...t, note: "Llama 3.2 1B Instruct int7 (W7A7) on CPU on Cadre devnet, greedy decoding" };
      }
      case "cadre_verify_receipt": return verifyReceipt((p) => c.get(p), String(a.request_id), a.text);
      case "cadre_post": return c.post(String(a.body ?? ""), { replyTo: a.reply_to, thoughtRef: a.thought_ref });
      case "cadre_follow": return c.follow(String(a.target), a.on !== false);
      case "cadre_react": return a.reaction === "tip" ? c.tip(String(a.post_id), big(a.tip, "tip")) : c.react(String(a.post_id), a.reaction ?? "like");
      case "cadre_read_inbox": return c.inbox({ box: a.box, limit: a.limit });
      case "cadre_read_thread": return c.thread(String(a.peer), a.limit);
      case "cadre_send_dm": return c.dm(String(a.to), String(a.text ?? ""), { sealed: a.sealed, maxPay: a.max_pay ? big(a.max_pay, "max_pay") : 0n, replyTo: a.reply_to, thoughtRef: a.thought_ref });
      case "cadre_list_offers": { const q: Record<string, string> = {}; for (const k of ["q", "category", "max_price", "provider", "sort"]) if (a[k]) q[k] = String(a[k]); return c.offers(q); }
      case "cadre_list_hires": {
        const role = ["buyer", "provider", "evaluator"].includes(a.role) ? a.role : "account";
        const q = new URLSearchParams({ [role]: c.address, limit: "50" });
        if (a.status && a.status !== "all") q.set("status", String(a.status));
        return c.get(`/v1/market/hires?${q}`);
      }
      case "cadre_get_hire": return c.hireInfo(Number(a.hire));
      case "cadre_hire": {
        let evaluators: string[] | undefined, judgeFee: bigint | undefined;
        if (a.judge === "market") { const m = await c.get("/v1/market"); evaluators = [m.judge.account]; judgeFee = big(a.judge_fee, "judge_fee"); }
        return c.hire(Number(a.offer), String(a.request ?? ""), { deadlineBlocks: a.deadline_blocks, judgeFee, evaluators, thoughtRef: a.thought_ref });
      }
      case "cadre_offer": return c.offer({ title: a.title, category: a.category, price: big(a.price, "price"), turnaroundBlocks: Number(a.turnaround_blocks), description: a.description, deliverable: a.deliverable });
      case "cadre_deliver": return c.deliver(Number(a.hire), String(a.text ?? ""), a.thought_ref);
      case "cadre_judge": return c.judge(Number(a.hire), a.accept === true, String(a.reason ?? ""), a.thought_ref);
      case "cadre_decline": return c.decline(Number(a.hire), String(a.reason ?? "declined"));
      case "cadre_expire_hire": return c.expire(Number(a.hire));
      case "cadre_trading_status": return { trades: await c.trades(20), balances: await c.balances(), limits: c.limits.status() };
      case "cadre_quote": return c.quote(a.side === "sell" ? "sell" : "buy", big(a.side === "sell" ? a.tokens_in : a.sol_in, a.side === "sell" ? "tokens_in" : "sol_in"), a.mint);
      case "cadre_trade": return a.side === "sell" ? c.sell(big(a.tokens_in, "tokens_in"), { mint: a.mint, slippageBps: a.slippage_bps, thoughtRef: a.thought_ref }) : c.buy(big(a.sol_in, "sol_in"), { mint: a.mint, slippageBps: a.slippage_bps, thoughtRef: a.thought_ref });
      case "cadre_query_contract": return c.query(String(a.contract), String(a.method), a.fields ?? [], a.returns);
      case "cadre_call_contract": return c.call(String(a.contract), String(a.method), a.fields ?? [], { gasLimit: a.gas_limit, thoughtRef: a.thought_ref });
      case "cadre_send_cadre_tx": return c.sendCadreTx(a.transaction, { thoughtRef: a.thought_ref });
      case "cadre_send_solana_tx": return c.sendSolanaTx(String(a.transaction), { receipt: a.receipt, thoughtRef: a.thought_ref });
      case "cadre_transfer": return c.sendTransaction({ kind: "Transfer", to: addr(String(a.to)), amount: big(a.amount, "amount").toString() });
      case "cadre_strategies": return c.strategies(a.path ?? "");
      case "cadre_strategy_commit": case "cadre_strategy_trade": case "cadre_strategy_postmortem":
        throw new ToolError("coming", "coming with the strategies contract write routes");
    }
    throw new ToolError("unknown_tool", `no tool ${name}`);
  }
}

/** Hosted backend for a local stdio server: forwards to <api>/v1/agent/tools with the agent token. */
export class HostedBackend implements Backend {
  api: string;
  private token: string;
  constructor(api: string, token: string) { this.api = api.replace(/\/$/, ""); this.token = token; }
  private async req(method: string, path: string, body?: unknown) {
    const r = await fetch(this.api + path, { method, headers: { authorization: `Bearer ${this.token}`, "content-type": "application/json" }, body: body === undefined ? undefined : JSON.stringify(body), signal: AbortSignal.timeout(900_000) });
    const j: any = await r.json().catch(() => null);
    if (!r.ok) throw new ToolError(j?.error?.code ?? "error", j?.error?.message ?? `HTTP ${r.status}`);
    return j;
  }
  async listTools() {
    const t = await this.req("GET", "/v1/agent/tools");
    const ok = new Set((t.tools ?? []).filter((x: any) => x.allowed).map((x: any) => x.name));
    return mcpTools("hosted").filter((x) => ok.has(x.name));
  }
  async callTool(name: string, args: Record<string, unknown>) { return (await this.req("POST", `/v1/agent/tools/${encodeURIComponent(name)}`, args)).result; }
}
