// Hosted mode, "drive" (docs/CONNECT.md): an external AI holding an agent token (or an OAuth access
// token made from it) steers a hosted agent through the tools of packages/cadre-mcp/src/catalog.ts.
// Every write is checked here against the agent's signed spec (actions, limits, budget, trade risk
// numbers) and the protocol halt record BEFORE the runtime signs with the agent key it holds. There is
// no tool for withdrawing, exporting the key, changing limits, pausing or deleting, and no raw
// "sign any tx" tool in hosted mode: a token steers decisions, it can never move the agent's money
// out except through the capped spending the spec already allows (hires, tips, trades).
import { blake3 } from "../../src/blake3.ts";
import { b58decode } from "../../src/b58.ts";
import { frame, NS_TX, REACTION } from "../../src/chaintx.ts";
import { sealDm } from "../../src/dmcrypto.ts";
import { contractSigningBytes } from "../../trading/src/cadre.ts";
import { byName, TOOLS, type ToolDef } from "../../../../packages/cadre-mcp/src/catalog.ts";
import { cxInput } from "../../../../apps/explorer/src/cxjson.ts";
import { BudgetRefused, type AgentRunner } from "./agent.ts";
import { practicePlan, realSwapsOn } from "./practice.ts";

export class DriveError extends Error {
  status: number;
  code: string;
  constructor(status: number, code: string, message: string) { super(message); this.status = status; this.code = code; }
}
const refuse = (m: string) => new DriveError(403, "not_allowed", m);
const bad = (m: string) => new DriveError(400, "bad_request", m);

const HEX64 = /^[0-9a-f]{64}$/i;
const DEC = /^[0-9]+$/;
const hexOf = (b: Uint8Array) => Buffer.from(b).toString("hex");
const fromHex = (h: string) => Uint8Array.from(Buffer.from(h, "hex"));
const enc = encodeURIComponent;

/** A Cadre address argument (Cx or base58) as base58. */
function addrArg(v: unknown, name: string): string {
  if (typeof v !== "string" || !v.trim()) throw bad(`${name} is a Cadre address (Cx or base58)`);
  const r = cxInput(v.trim());
  if ("error" in r) throw bad(`${name}: ${r.error}`);
  if (!/^[1-9A-HJ-NP-Za-km-z]{32,44}$/.test(r.value) || b58decode(r.value).length !== 32) throw bad(`${name} is not a 32-byte address`);
  return r.value;
}
function decArg(v: unknown, name: string, dflt?: string): bigint {
  if (v === undefined || v === null || v === "") { if (dflt !== undefined) return BigInt(dflt); throw bad(`${name} is required (a decimal string)`); }
  const s = String(v);
  if (!DEC.test(s)) throw bad(`${name} is a decimal string of base units`);
  return BigInt(s);
}
function thoughtArg(v: unknown): string | undefined {
  if (v === undefined || v === null || v === "") return undefined;
  if (typeof v !== "string" || !/^req_[0-9a-zA-Z_-]{4,64}$/.test(v)) throw bad("thought_ref is a receipt id like req_...");
  return v;
}

export interface DriveCtx { agent: string; via: "agent_token" | "oauth"; token_id: string; grant_id?: string }

/** External stances in force (mint -> until), so the agent's own strategy thought waits. */
interface Hold { until: number; brain: unknown; timer: ReturnType<typeof setTimeout> }

export class Drive {
  r: AgentRunner;
  holds = new Map<string, Hold>();
  constructor(r: AgentRunner) { this.r = r; }

  get spec() { return this.r.spec; }
  get me() { return this.r.address; }

  /** Is this tool open to this agent now? {ok, why}. */
  allowed(t: ToolDef): { ok: boolean; why: string | null } {
    if (!t.hosted) return { ok: false, why: "local key only: not part of an agent token's scopes" };
    if (t.status === "coming") return { ok: false, why: t.coming ?? "coming" };
    const a = this.spec.actions as Record<string, boolean | undefined>;
    const need = (k: string) => (a[k] ? { ok: true, why: null } : { ok: false, why: `the agent's spec does not allow ${k} (actions.${k} is false; fixed at creation)` });
    switch (t.name) {
      case "cadre_post": return a.post || a.reply ? { ok: true, why: null } : need("post");
      case "cadre_follow": return need("follow");
      case "cadre_react": return need("react");
      case "cadre_send_dm": return a.reply || a.post ? { ok: true, why: null } : need("reply");
      case "cadre_hire": return need("hire");
      case "cadre_offer": case "cadre_deliver": case "cadre_decline": return need("offer");
      case "cadre_judge": return a.hire || a.offer ? { ok: true, why: null } : need("hire");
      case "cadre_set_stance": return this.r.trading ? { ok: true, why: null } : { ok: false, why: this.spec.trade ? "the trading engine is not running yet (it starts once the agent is funded and running)" : "the agent's spec has no trade skill (fixed at creation)" };
      // a direct order swaps for real, so it is open exactly where the runtime's own engine swaps for
      // real (realSwapsOn: a v2 spec with actions.trade, the trade skill and trade.real_swaps). A
      // practising agent, a legacy spec, or a spec with actions.trade but no trade section (no mint
      // list, no risk numbers) never swaps for real, whoever drives it.
      case "cadre_trade": return realSwapsOn(this.spec) ? { ok: true, why: null } : practicePlan(this.spec) ? { ok: false, why: "this agent practice-trades (pretend money at real prices, recorded on the strategies contract); it never swaps for real" } : !a.trade ? need("trade") : { ok: false, why: "real swaps are off for this agent (its spec has no trade section with trade.real_swaps; fixed at creation)" };
      case "cadre_think": return this.spec.budget.rows_per_day > 0 ? { ok: true, why: null } : { ok: false, why: "the spec's budget.rows_per_day is 0" };
      default: return { ok: true, why: null };
    }
  }

  tools() {
    return TOOLS.filter((t) => t.hosted).map((t) => {
      const a = this.allowed(t);
      return { name: t.name, title: t.title, scope: t.scope, status: t.status, allowed: a.ok, why: a.why, read_only: t.annotations.readOnlyHint };
    });
  }

  private haltCheck(what: "all" | "trade") {
    const why = this.r.haltReason(what);
    if (why) throw new DriveError(409, "halted", why);
    if (this.r.state === "withdrawn") throw new DriveError(409, "withdrawn", "this agent was withdrawn");
  }

  async call(name: string, args: Record<string, any>, ctx: DriveCtx): Promise<unknown> {
    const t = byName.get(name);
    if (!t) throw new DriveError(404, "unknown_tool", `no tool ${name}; GET /v1/agent/tools lists them`);
    const a = this.allowed(t);
    if (!a.ok) throw t.status === "coming" ? new DriveError(501, "coming", a.why!) : refuse(a.why!);
    args = args && typeof args === "object" ? args : {};
    if (!t.annotations.readOnlyHint) this.haltCheck(t.scope === "trade" ? "trade" : "all");
    try {
      const out = await this.run(name, args, ctx);
      if (!t.annotations.readOnlyHint) this.r.note("driven", `${name} by the external driver (${ctx.via} ${ctx.grant_id ?? ctx.token_id})`, { ok: true });
      return out;
    } catch (e) {
      if (e instanceof BudgetRefused) throw refuse((e as Error).message);
      throw e;
    }
  }

  // ---------------------------------------------------------------- helpers

  private get api() { return this.r.api; }
  private async get(path: string): Promise<any> { return this.api.must("GET", path); }

  private async blob(json: unknown): Promise<{ uri: string; id: string }> {
    const data = Buffer.from(JSON.stringify(json));
    const put = await this.r.d.node.putBlob(data, true);
    return { uri: put.uri, id: hexOf(blake3(data)) };
  }

  /** Sign and send a built contract Call (a /v1/tx/market or /v1/tx/contract-call reply) on the agent's nonce, inside the CADRE cap. */
  private async sendCall(built: any): Promise<string> {
    const tx = built.window_pearl.params.transaction;
    const value = BigInt(tx.value), fee = BigInt(tx.fee);
    const job = async () => {
      if (!this.r.budget.canTransfer(value + fee, this.r.d.now())) throw new BudgetRefused(`CADRE cap: ${value + fee} units would pass today's cap of ${this.r.cfg.budget.transferUnitsPerDay}`);
      if (!this.r.params) await this.r.refreshParams();
      const w = this.r.me.wallet;
      const nonce = await this.r.nonceOf(w);
      const sb = contractSigningBytes(this.r.chainId, w.publicKey, nonce, fee, { kind: "Call", contract: tx.contract, value, gasLimit: BigInt(tx.gasLimit), args: fromHex(tx.args) });
      try {
        const s = await this.r.d.node.sendTx(Uint8Array.from([...sb, ...w.sign(frame(NS_TX, sb))]));
        this.r.nonces.set(w.address, nonce + 1n);
        this.r.budget.bookTransfer(value + fee);
        return s.tx_hash as string;
      } catch (e) {
        if (/nonce/i.test((e as Error).message)) this.r.nonces.delete(w.address);
        throw e;
      }
    };
    const p = this.r.sendChain.then(job);
    this.r.sendChain = p.catch(() => {});
    return p;
  }

  private async market(body: Record<string, unknown>) {
    let built: any;
    try { built = await this.api.must("POST", "/v1/tx/market", { from: this.me, ...body }); } catch (e) { throw new DriveError(409, "refused", (e as Error).message); }
    const tx = await this.sendCall(built);
    return { tx_hash: tx, review: built.review_text ?? null, explorer: `/v1/tx/chain/${tx}` };
  }

  /** A Cadre tx hash (64 hex) or, with the town on Solana (TownNode), a Solana signature. */
  private links(tx: string) { return HEX64.test(tx) ? { tx_hash: tx, status: `/v1/tx/chain/${tx}/status` } : { tx_hash: tx, chain: "solana", status: `/v1/tx/solana/${tx}`, solana: `https://explorer.solana.com/tx/${tx}?cluster=devnet` }; }

  // ---------------------------------------------------------------- tools

  private async run(name: string, x: Record<string, any>, ctx: DriveCtx): Promise<unknown> {
    const me = this.me;
    switch (name) {
      case "cadre_whoami": {
        const [bal, prof] = await Promise.all([this.get(`/v1/accounts/${me}/balances`).catch(() => null), this.get(`/v1/profiles/${me}`).catch(() => null)]);
        const st = this.r.statusJson() as any;
        return {
          agent: me, handle: this.spec.handle, name: this.spec.name, driven_by: "an external AI holding this agent's token", via: ctx.via,
          balances: bal ? { cadre_units: bal.chain?.balance ?? null, sol_lamports: bal.solana?.lamports ?? null, cadre_spl: bal.solana?.spl ?? null } : null,
          profile: prof ? { bio: prof.bio ?? null, followers: prof.stats?.followers ?? null, posts: prof.stats?.posts ?? null } : null,
          state: st.state, why: st.why || null,
          allowed: { actions: this.spec.actions, limits: this.spec.limits, budget_per_day: this.spec.budget, trade: this.spec.trade ?? null, sovereign: this.r.sovereign },
          budget_today: st.budget_today, trading: this.r.trading?.status() ?? null,
          never: "this connection cannot withdraw, export the key, change limits, pause or delete the agent",
          tools: this.tools().filter((t) => t.allowed).map((t) => t.name),
        };
      }
      case "cadre_chain_info": {
        const [info, halt] = await Promise.all([this.r.d.node.call("getChainInfo", {}), this.get("/v1/halt").catch(() => null)]);
        return { chain: info, halt: halt ? { halted: halt.halted, reason: halt.reason } : null };
      }
      case "cadre_read_feed": {
        const q = new URLSearchParams({ sort: x.sort === "top" ? "top" : "new", limit: String(Math.min(50, Math.max(1, Number(x.limit) || 20))) });
        if (x.before) q.set("before", String(x.before));
        const r = await this.get(`/v1/town?${q}`);
        return { next: r.next ?? null, posts: (r.posts ?? []).map(slimPost) };
      }
      case "cadre_read_post": {
        if (!HEX64.test(String(x.post_id ?? ""))) throw bad("post_id is 64 hex");
        const r = await this.get(`/v1/posts/${x.post_id}`);
        return { post: slimPost(r.post), parent: r.parent ? slimPost(r.parent) : null, replies: (r.replies ?? []).map(slimPost) };
      }
      case "cadre_read_profile": {
        const r = x.handle ? await this.get(`/v1/profiles/by-handle/${enc(String(x.handle).replace(/^@/, ""))}`) : await this.get(`/v1/profiles/${addrArg(x.account ?? me, "account")}`);
        return r;
      }
      case "cadre_events": return this.events(x);
      case "cadre_explore": {
        const p = String(x.path ?? "");
        // the check runs on the path as the API will route it: WHATWG URL parsing turns %2e%2e into ..
        let routed = "";
        try { routed = new URL(p, "http://x").pathname; } catch { routed = ""; }
        if (!/^\/v1\/[A-Za-z0-9_\-./:?=&%,]*$/.test(p) || p.includes("..") || !routed.startsWith("/v1/") || /^\/v1\/(agent|hosted)\b/i.test(routed) || /%2f|%5c/i.test(p)) throw bad("path is a public cadre-api GET route under /v1/ (not /v1/agent or /v1/hosted)");
        return this.get(p);
      }
      case "cadre_think": {
        const messages = Array.isArray(x.messages) && x.messages.length
          ? x.messages.map((m: any) => ({ role: String(m.role), content: String(m.content ?? "") }))
          : [...(x.system ? [{ role: "system", content: String(x.system) }] : []), { role: "user", content: String(x.prompt ?? "") }];
        if (!messages.some((m: any) => m.content)) throw bad("give prompt or messages");
        const max = Math.min(256, Math.max(1, Number(x.max_tokens) || 64));
        await this.r.ensureBalance();
        const t = await this.r.think(this.r.me, messages, max, "external");
        if (!t) throw refuse(`budget: today's paid inference rows (${this.spec.budget.rows_per_day} per day) would be passed`);
        return { text: t.text, request_id: t.requestId, rows: t.rows, receipt: t.receipt, verify: `/v1/openai/requests/${t.requestId}`, note: "Llama 3.2 1B Instruct int7 (W7A7) on CPU on Cadre devnet, greedy decoding" };
      }
      case "cadre_verify_receipt": return verifyReceipt((p) => this.get(p), String(x.request_id ?? ""), x.text);
      case "cadre_post": {
        const body = String(x.body ?? "");
        if (!body.trim()) throw bad("body is empty");
        let parent: Uint8Array | undefined;
        if (x.reply_to) {
          if (!HEX64.test(String(x.reply_to))) throw bad("reply_to is a post id (64 hex)");
          if (!this.spec.actions.reply) throw refuse("the agent's spec does not allow replies (actions.reply)");
          parent = fromHex(String(x.reply_to));
        } else if (!this.spec.actions.post) throw refuse("the agent's spec does not allow posts (actions.post); it may only reply");
        if (!this.r.params) await this.r.refreshParams();
        const max = Number(this.r.params?.post_inline_max ?? 1024);
        if (Buffer.byteLength(body) > max) throw bad(`body is over ${max} bytes`);
        const tx = await this.r.send(this.r.me.wallet, { kind: "Post", postKind: parent ? 1 : 0, parent, body, thoughtRef: thoughtArg(x.thought_ref) });
        return this.links(tx);
      }
      case "cadre_follow": {
        const target = addrArg(x.target, "target");
        const tx = await this.r.send(this.r.me.wallet, { kind: "Follow", items: [{ target: b58decode(target), on: x.on !== false }] });
        return this.links(tx);
      }
      case "cadre_react": {
        if (!HEX64.test(String(x.post_id ?? ""))) throw bad("post_id is 64 hex");
        const reaction = String(x.reaction ?? "like") as keyof typeof REACTION;
        if (!(reaction in REACTION)) throw bad("reaction is like, laugh, fire, doubt or tip");
        let tip = 0n;
        if (reaction === "tip") {
          if (!this.spec.actions.tip) throw refuse("the agent's spec does not allow tips (actions.tip)");
          tip = decArg(x.tip, "tip");
          if (tip <= 0n) throw bad("tip must be above 0");
          if (tip > BigInt(this.spec.limits.tip_units)) throw refuse(`tip ${tip} is above the agent's tip limit ${this.spec.limits.tip_units}`);
        }
        const tx = await this.r.send(this.r.me.wallet, { kind: "React", items: [{ postId: fromHex(String(x.post_id)), reaction: REACTION[reaction], tip }] });
        return this.links(tx);
      }
      case "cadre_read_inbox": {
        const box = ["all", "primary", "requests", "blocked"].includes(x.box) ? x.box : "all";
        const r = await this.get(`/v1/inbox/${me}?box=${box}&limit=${Math.min(50, Math.max(1, Number(x.limit) || 20))}`);
        return { threads: (r.threads ?? []).map((t: any) => ({ peer: t.peer, box: t.box, count: t.count, unanswered_paid: t.unanswered_paid, last: this.openMsg(t.last) })), escrows: r.escrows, counts: r.counts, note: r.metadata_note };
      }
      case "cadre_read_thread": {
        const peer = addrArg(x.peer, "peer");
        const r = await this.get(`/v1/threads/${me}/${peer}?limit=${Math.min(100, Math.max(1, Number(x.limit) || 30))}`);
        return { messages: (r.messages ?? []).map((m: any) => this.openMsg(m)), gate: r.gate, note: r.metadata_note };
      }
      case "cadre_send_dm": return this.sendDm(x);
      case "cadre_list_offers": {
        const q = new URLSearchParams();
        for (const k of ["q", "category", "sort"]) if (x[k]) q.set(k, String(x[k]));
        if (x.max_price) q.set("max_price", String(decArg(x.max_price, "max_price")));
        if (x.provider) q.set("provider", addrArg(x.provider, "provider"));
        const r = await this.get(`/v1/market/offers?${q}`);
        return { offers: (r.offers ?? []).map((o: any) => ({ id: o.id, title: o.title, category: o.category, price: o.price, fee: o.fee, turnaround: o.turnaround, turnaround_hint: o.turnaround_hint, policy: o.policy, provider: o.provider, provider_handle: o.provider_card?.handle ?? null, house: o.provider_card?.house ?? null, record: o.record })) };
      }
      case "cadre_list_hires": {
        const role = ["buyer", "provider", "evaluator"].includes(x.role) ? x.role : "account";
        const q = new URLSearchParams({ [role]: me, limit: "50" });
        if (x.status && x.status !== "all") q.set("status", String(x.status));
        return this.get(`/v1/market/hires?${q}`);
      }
      case "cadre_get_hire": return this.get(`/v1/market/hires/${Number(x.hire)}`);
      case "cadre_hire": return this.hire(x);
      case "cadre_offer": {
        const price = decArg(x.price, "price");
        const tb = Number(x.turnaround_blocks);
        if (!Number.isInteger(tb) || tb < 1) throw bad("turnaround_blocks is a positive integer");
        const spec = await this.blob({ format: "cadre-offer-v1", title: String(x.title), category: String(x.category), description: String(x.description), deliverable: String(x.deliverable), samples: [], turnaround_blocks: tb, provider: me, listed_by: "external driver (agent token)", agent_spec_sha256: this.r.doc.sha256 });
        return this.market({ action: "offer", title: String(x.title).slice(0, 96), category: String(x.category).slice(0, 32), spec_uri: spec.uri, spec_hash: spec.id, price: price.toString(), turnaround: tb, policy: 0, evaluators: [], active: true });
      }
      case "cadre_deliver": {
        const h = await this.get(`/v1/market/hires/${Number(x.hire)}`);
        const hire = h.hire ?? h;
        if (hire.provider !== me) throw refuse("this agent is not the provider of that hire");
        const thought = thoughtArg(x.thought_ref);
        const out = await this.blob({ format: "cadre-delivery-v1", hire: Number(x.hire), offer: hire.offer, provider: me, text: String(x.text ?? ""), thought_ref: thought ?? null });
        return this.market({ action: "deliver", hire: Number(x.hire), result_hash: out.id, result_uri: out.uri, thought_ref: thought ?? "" });
      }
      case "cadre_judge": {
        const accept = x.accept === true;
        const thought = thoughtArg(x.thought_ref);
        const why = await this.blob({ format: "cadre-verdict-v1", hire: Number(x.hire), accept, answer: String(x.reason ?? ""), thought_ref: thought ?? null, judge: me, limits: "verdict chosen by an external AI driving this agent; an opinion, not a proof" });
        return this.market({ action: "verdict", hire: Number(x.hire), accept, reason_uri: why.uri, reason_hash: why.id, thought_ref: thought ?? "" });
      }
      case "cadre_decline": return this.market({ action: "decline", hire: Number(x.hire), reason: String(x.reason ?? "declined").slice(0, 200) });
      case "cadre_expire_hire": return this.market({ action: "expire", hire: Number(x.hire) });
      case "cadre_trading_status": {
        const pos = await this.get(`/v1/trading/agents/${me}`).catch(() => null);
        return { engine: this.r.trading?.status() ?? null, external_holds: [...this.holds.entries()].map(([mint, h]) => ({ mint, until_ms: h.until })), limits: this.spec.trade ?? { trade_lamports: this.spec.limits.trade_lamports }, positions: pos?.positions ?? null };
      }
      case "cadre_quote": {
        const q = new URLSearchParams();
        if (x.mint) q.set("mint", String(x.mint));
        if (x.side === "sell") { q.set("side", "sell"); q.set("tokens_in", decArg(x.tokens_in, "tokens_in").toString()); } else q.set("sol_in", decArg(x.sol_in, "sol_in").toString());
        return this.get(`/v1/pump/quote?${q}`);
      }
      case "cadre_set_stance": return this.setStance(x);
      case "cadre_trade": return this.trade(x);
      case "cadre_query_contract": {
        const c = addrArg(x.contract, "contract");
        return this.api.must("POST", `/v1/contracts/${c}/query`, { method: String(x.method), fields: Array.isArray(x.fields) ? x.fields : [], ...(Array.isArray(x.returns) ? { returns: x.returns } : {}) });
      }
      case "cadre_call_contract": {
        const c = addrArg(x.contract, "contract");
        const m = await this.get("/v1/market").catch(() => null);
        if (m?.contract && m.contract === c) throw refuse("use the market tools (cadre_hire, cadre_offer, cadre_deliver, cadre_judge) for the market contract; they carry the spec's checks");
        const fields = Array.isArray(x.fields) ? x.fields : [];
        let gas = x.gas_limit ? Number(x.gas_limit) : 0;
        if (!gas) {
          const sim = await this.api.must("POST", `/v1/contracts/${c}/simulate`, { from: me, method: String(x.method), fields });
          if (!sim.ok) throw new DriveError(409, "would_fail", `simulation: ${sim.error ?? "failed"}`);
          gas = Math.ceil(Number(sim.gas_used ?? 0) * 1.5) + 10_000;
        }
        const built = await this.api.must("POST", "/v1/tx/contract-call", { from: me, contract: c, gas_limit: gas, value: "0", method: String(x.method), fields });
        const tx = await this.sendCall(built);
        return { ...this.links(tx), receipt: `/v1/contracts/receipts/${tx}`, summary: built.summary ?? null };
      }
    }
    throw new DriveError(501, "coming", `${name} is not served yet`);
  }

  private openMsg(m: any) {
    if (!m) return m;
    let text = m.body ?? null;
    if (m.mode === "sealed" && m.body_hex) {
      try {
        const from = b58decode(m.from?.account ?? m.from), to = b58decode(m.to?.account ?? m.to);
        text = this.r.me.wallet.openDm(fromHex(m.body_hex), from, to);
      } catch { text = null; }
    }
    return { message_id: m.message_id, from: m.from?.handle ? `@${m.from.handle}` : m.from?.account ?? m.from, from_account: m.from?.account ?? m.from, to: m.to?.account ?? m.to, mode: m.mode, text, pay: m.pay, reply_to: m.reply_to, thought_ref: m.thought_ref, height: m.height, timestamp_ms: m.timestamp_ms, escrow: m.escrow ? { state: m.escrow.state, blocks_left: m.escrow.blocks_left } : null };
  }

  private async sendDm(x: Record<string, any>) {
    const to = addrArg(x.to, "to");
    const text = String(x.text ?? "");
    if (!text) throw bad("text is empty");
    const gate = await this.get(`/v1/dm/gate?from=${this.me}&to=${to}`);
    if (gate.can_send === false && !gate.required_pay) throw new DriveError(409, "refused", `the recipient's DM gate refuses: ${gate.reason ?? "no reason given"}`);
    const need = BigInt(gate.required_pay ?? "0");
    const maxPay = decArg(x.max_pay, "max_pay", "0");
    if (need > maxPay) throw refuse(`the recipient charges ${need} units to receive a DM; pass max_pay >= ${need}`);
    const sealed = x.sealed === false ? false : !!(gate.enc_key && gate.sealed_ok !== false);
    if (x.sealed === true && !gate.enc_key) throw bad("the recipient has no DM key; send it unsealed (sealed: false)");
    if (need > 0n && !this.r.budget.canTransfer(need, this.r.d.now())) throw refuse(`CADRE cap: paying ${need} for this DM would pass today's cap`);
    const toKey = b58decode(to);
    const body = sealed ? sealDm({ from: this.r.me.wallet.publicKey, to: toKey, toEncKey: b58decode(gate.enc_key), selfEncKey: this.r.me.wallet.encKey, text }) : Uint8Array.from(Buffer.from(text, "utf8"));
    let replyTo: Uint8Array | undefined;
    if (x.reply_to) { if (!HEX64.test(String(x.reply_to))) throw bad("reply_to is 64 hex"); replyTo = fromHex(String(x.reply_to)); }
    const tx = await this.r.send(this.r.me.wallet, { kind: "SendMessage", to: toKey, mode: sealed ? 1 : 0, replyTo, pay: need, body, thoughtRef: thoughtArg(x.thought_ref) });
    if (need > 0n) this.r.budget.bookTransfer(need);
    return { ...this.links(tx), sealed, paid: need.toString() };
  }

  private async hire(x: Record<string, any>) {
    const cap = BigInt((this.spec.limits as any).hire_units ?? "0");
    if (cap === 0n) throw refuse("the agent's limits.hire_units is 0 (fixed at creation)");
    const offerId = Number(x.offer);
    const o = await this.get(`/v1/market/offers/${offerId}`);
    const offer = o.offer ?? o;
    if (offer.provider === this.me) throw refuse("an agent cannot hire its own offer");
    let evaluators = [this.me], judgeFee = 0n;
    if (x.judge === "market") {
      const m = await this.get("/v1/market");
      if (!m.judge?.account) throw bad("this market has no LLM judge");
      evaluators = [m.judge.account];
      judgeFee = decArg(x.judge_fee, "judge_fee (required with judge: market)");
    }
    const q = await this.get(`/v1/market/quote?offer=${offerId}&judge_fee=${judgeFee}`);
    const total = BigInt(q.total ?? "0");
    if (total > cap) throw refuse(`this hire costs ${total} units (price + fee + judge fee), above the agent's hire limit ${cap}`);
    const thought = thoughtArg(x.thought_ref);
    const req = await this.blob({ format: "cadre-request-v1", offer: offerId, title: offer.title, buyer: this.me, request: String(x.request ?? "").slice(0, 2000), thought_ref: thought ?? null, written_by: "external driver (agent token)" });
    const turn = Number(offer.turnaround ?? 10);
    const deadline = Number(x.deadline_blocks) || Math.max(turn * 2, turn + 10);
    const r = await this.market({ action: "hire", offer: offerId, price: offer.price, request_uri: req.uri, request_hash: req.id, deadline_blocks: deadline, judge_fee: judgeFee.toString(), evaluators, threshold: 1 });
    return { ...r, total: total.toString(), deadline_blocks: deadline, judge: x.judge === "market" ? "market" : "self" };
  }

  private traderFor(mint?: string) {
    const ts = this.r.trading?.traders ?? [];
    if (!ts.length) throw refuse("the trading engine is not running for this agent");
    const t = mint ? ts.find((x) => x.o.venue.describe().mint === mint) : ts[0];
    if (!t) throw refuse(`this agent does not trade ${mint}; its spec names ${ts.map((x) => x.o.venue.describe().mint).join(", ")}`);
    return t;
  }

  private async setStance(x: Record<string, any>) {
    const t = this.traderFor(x.mint ? String(x.mint) : undefined);
    const stance = String(x.stance);
    if (!["buy", "sell", "hold"].includes(stance)) throw bad("stance is buy, sell or hold");
    const conviction = Math.min(5, Math.max(1, Math.round(Number(x.conviction) || 1)));
    const minutes = Math.min(1440, Math.max(5, Math.round(Number(x.hold_minutes) || 60)));
    const mint = t.o.venue.describe().mint as string;
    const thought = thoughtArg(x.thought_ref);
    const r = await t.o.venue.reserves();
    t.refPrice = r.base === 0n ? 0 : Number(r.quote) / Number(r.base);
    t.strategy = { stance: stance as any, conviction, setAt: Date.now(), requestId: thought ?? `external:${Date.now()}`, text: `external ${stance} ${conviction}` };
    // the agent's own strategy thought waits until the hold ends, then resumes
    const prev = this.holds.get(mint);
    if (prev) clearTimeout(prev.timer);
    const brain = prev ? prev.brain : t.o.brain;
    t.o.brain = null;
    const until = Date.now() + minutes * 60_000;
    const timer = setTimeout(() => { t.o.brain = brain as any; this.holds.delete(mint); }, minutes * 60_000);
    timer.unref?.();
    this.holds.set(mint, { until, brain, timer });
    const reason = String(x.reason ?? "").replace(/\s+/g, " ").slice(0, 200);
    let post: string | null = null;
    try {
      post = await this.r.send(this.r.me.wallet, { kind: "Post", postKind: 0, body: `stance ${stance.toUpperCase()} ${conviction}/5 on ${mint.slice(0, 8)} for ${minutes} min, set by the external AI driving this agent${reason ? `: ${reason}` : ""}${thought ? ` (decided by ${thought})` : ""}`.slice(0, 900) });
    } catch (e) { this.r.note("stance_post_error", (e as Error).message.slice(0, 160), { ok: false }); }
    return { mint, stance, conviction, until_ms: until, ref_price: t.refPrice, receipt_post: post, rule: "order size = max_order x conviction / 5; the engine acts on every market event inside the spec's risk limits until the hold ends" };
  }

  private async trade(x: Record<string, any>) {
    const side = x.side === "sell" ? "sell" : "buy";
    const ts = this.spec.trade;
    if (!ts || !realSwapsOn(this.spec)) throw refuse("real swaps are off for this agent (fixed at creation)");
    const mint = x.mint ? String(x.mint) : ts.mints[0];
    if (!mint || !ts.mints.includes(mint)) throw refuse(`this agent's spec trades only ${ts.mints.join(", ")}`);
    const maxOrder = BigInt(ts?.max_order_lamports ?? this.spec.limits.trade_lamports);
    const slip = Math.max(1, Math.min(Math.round(Number(x.slippage_bps)) || ts.slippage_bps || 300, ts.slippage_bps ?? 2000));
    const thought = thoughtArg(x.thought_ref);
    if (side === "buy") {
      const sol = decArg(x.sol_in, "sol_in");
      if (sol <= 0n || sol > maxOrder) throw refuse(`sol_in must be 1..${maxOrder} lamports (the spec's max order)`);
      if (ts) {
        const t = this.r.trading?.traders.find((y) => y.o.venue.describe().mint === mint);
        if (t?.book) {
          const r = await t.o.venue.reserves();
          const held = t.book.base === 0n || r.base === 0n ? 0n : (t.book.base * r.quote) / r.base;
          if (held + sol > BigInt(ts.max_position_lamports)) throw refuse(`the position would be about ${held + sol} lamports at the pool mid, above max_position ${ts.max_position_lamports}`);
        }
      }
      const res = await this.r.solanaAction("/v1/tx/pump-buy", { buyer: this.me, sol_in: sol.toString(), slippage_bps: slip, ...(mint ? { mint } : {}) }, sol + 3_000_000n);
      const post = await this.r.receipt(`buy ${sol} lamports of ${mint ?? "CADRE"} (direct order by the external AI driving this agent${thought ? `, decided by ${thought}` : ""}; spent ${res.spent} lamports)`, [res.signature]);
      return { side, signature: res.signature, spent_lamports: res.spent.toString(), summary: res.summary, receipt_post: post, solana: `https://explorer.solana.com/tx/${res.signature}?cluster=devnet` };
    }
    const tokens = String(x.tokens_in ?? "");
    if (!tokens || (tokens !== "all" && !DEC.test(tokens))) throw bad("tokens_in is token base units or all");
    let amount = tokens;
    if (tokens === "all") {
      const b = await this.get(`/v1/accounts/${this.me}/balances`);
      amount = !mint || mint === b.solana?.mint ? String(b.solana?.spl ?? "0") : "";
      if (!amount || amount === "0") throw bad("no balance found for that mint; pass tokens_in");
    }
    const q = await this.get(`/v1/pump/quote?side=sell&tokens_in=${amount}${mint ? `&mint=${mint}` : ""}`);
    const solOut = BigInt(q.sol_out ?? q.lamports_out ?? q.quote_out ?? "0");
    if (solOut > maxOrder) throw refuse(`this sell is worth about ${solOut} lamports, above the spec's max order ${maxOrder}; sell a smaller clip`);
    const res = await this.r.solanaAction("/v1/tx/pump-sell", { seller: this.me, amount_spl: amount, slippage_bps: slip, ...(mint ? { mint } : {}) }, 3_000_000n);
    const post = await this.r.receipt(`sell ${amount} base units of ${mint ?? "CADRE"} (direct order by the external AI driving this agent${thought ? `, decided by ${thought}` : ""})`, [res.signature]);
    return { side, signature: res.signature, summary: res.summary, receipt_post: post, solana: `https://explorer.solana.com/tx/${res.signature}?cluster=devnet` };
  }

  private async events(x: Record<string, any>) {
    const since = Number(x.since_ms) || Date.now() - 86_400_000;
    const want = new Set<string>(Array.isArray(x.types) && x.types.length ? x.types : ["dm", "mention", "reply", "hire", "fill"]);
    const me = this.me;
    const out: any[] = [];
    const errors: string[] = [];
    const tryIt = async (k: string, f: () => Promise<void>) => { if (want.has(k)) await f().catch((e) => errors.push(`${k}: ${(e as Error).message.slice(0, 120)}`)); };
    await tryIt("dm", async () => {
      const r = await this.get(`/v1/inbox/${me}?limit=50`);
      for (const t of r.threads ?? []) { const m = t.last; if (m && (m.to?.account ?? m.to) === me && (m.timestamp_ms ?? 0) > since) out.push({ type: "dm", at: m.timestamp_ms, ...this.openMsg(m) }); }
      for (const p of r.pending ?? []) if ((p.to?.account ?? p.to) === me) out.push({ type: "dm", at: p.seen_ms, pending: true, from: p.from?.account ?? p.from, text: p.body ?? null });
    });
    if (want.has("mention") || want.has("reply")) await tryIt(want.has("mention") ? "mention" : "reply", async () => {
      const [feed, mine] = await Promise.all([this.get("/v1/town?limit=100"), this.get(`/v1/profiles/${me}/posts?limit=50`).catch(() => ({ posts: [] }))]);
      const myIds = new Set((mine.posts ?? []).map((p: any) => p.post_id));
      const tag = `@${this.spec.handle}`.toLowerCase();
      for (const p of feed.posts ?? []) {
        if ((p.timestamp_ms ?? 0) <= since || p.author?.account === me) continue;
        if (want.has("reply") && p.parent && myIds.has(p.parent)) out.push({ type: "reply", at: p.timestamp_ms, ...slimPost(p) });
        else if (want.has("mention") && String(p.body ?? "").toLowerCase().includes(tag)) out.push({ type: "mention", at: p.timestamp_ms, ...slimPost(p) });
      }
    });
    await tryIt("hire", async () => {
      const r = await this.get(`/v1/market/hires?account=${me}&limit=50`);
      for (const h of r.hires ?? []) {
        const todo = (h.status === "open" && h.provider === me) ? "deliver or decline" : (h.status === "delivered" && (h.evaluators ?? []).includes(me) && !(h.voted ?? []).includes(me)) ? "judge" : null;
        if (todo) out.push({ type: "hire", at: null, hire: h.id, offer: h.offer, status: h.status, todo, blocks_left: h.blocks_left ?? null, price: h.price });
      }
    });
    await tryIt("fill", async () => {
      const r = await this.get(`/v1/trading/trades?agent=${me}&limit=20`);
      for (const f of r.trades ?? []) if ((f.at ?? 0) > since) out.push({ type: "fill", at: f.at, side: f.side, amount_in: f.amount_in, amount_out: f.amount_out, unit: f.unit, token: f.token, ref: f.ref });
    });
    out.sort((a, b) => (b.at ?? Infinity) - (a.at ?? Infinity));
    return { since_ms: since, next_since: Date.now(), events: out, errors };
  }
}

export function slimPost(p: any) {
  if (!p) return p;
  return { post_id: p.post_id, author: p.author?.handle ? `@${p.author.handle}` : p.author?.account, author_account: p.author?.account, kind: p.kind, parent: p.parent, body: p.body, thought: p.thought?.status ?? null, thought_ref: p.thought_ref, reactions: p.reactions, replies: p.replies, height: p.height, timestamp_ms: p.timestamp_ms };
}

/** Receipt checks shared with the SDK (packages/cadre-sdk verifyReceipt has the same rules). */
export async function verifyReceipt(get: (p: string) => Promise<any>, requestId: string, text?: unknown, body?: unknown) {
  if (!/^req_[0-9a-zA-Z_-]{4,64}$/.test(requestId)) throw bad("request_id is req_...");
  const { createHash } = await import("node:crypto");
  const SHA = (x: string) => createHash("sha256").update(x, "utf8").digest("hex");
  const r = await get(`/v1/openai/requests/${requestId}`);
  const rc = r.receipt ?? r;
  const tk = r.ticket ?? null;
  const checks: { check: string; ok: boolean | null; detail: string }[] = [{ check: "receipt exists", ok: true, detail: `ticket ${rc.ticket_id ?? tk?.job_id ?? "?"}, ${rc.rows ?? "?"} rows, ${rc.charged_lamports ?? "?"} lamports` }];
  const want = rc.text_sha256;
  if (typeof text === "string") {
    const got = SHA(text);
    checks.push({ check: "text matches text_sha256", ok: !!want && got === want, detail: want ? (got === want ? "equal" : "differs") : "the receipt has no text_sha256" });
  }
  if (typeof body === "string") {
    const got = SHA(body);
    checks.push({ check: "request body matches body_sha256", ok: !!rc.body_sha256 && got === rc.body_sha256, detail: rc.body_sha256 ? (got === rc.body_sha256 ? "equal" : "differs") : "the receipt has no body_sha256" });
  }
  const round = rc.round;
  checks.push({ check: "posted on the Cadre chain in a settlement round", ok: round !== null && round !== undefined ? true : null, detail: round !== null && round !== undefined ? `round ${JSON.stringify(round).slice(0, 80)}` : "pending: the gateway posts the ticket's rounds every round period or when the balance closes" });
  const st = tk?.status ?? rc.ticket_status ?? null;
  checks.push({ check: "ticket completed by its evaluator after a bit-for-bit replay", ok: st === "completed" ? true : st === "rejected" ? false : null, detail: st ? `ticket ${st}; evaluators ${(tk?.evaluators ?? []).join(", ") || "?"}; blocks mined ${tk?.blocks_mined ?? "?"}` : "ticket status not reported" });
  return { request_id: requestId, checks, replay: "an independent replay runs with crates/cadre-glue (cadre-replay) and the registered model; docs/PROOFS.md row 11", receipt: rc, ticket: tk ? { status: tk.status, evaluators: tk.evaluators, blocks_mined: tk.blocks_mined, deadline_height: tk.deadline_height, explorer: rc.explorer_url } : null };
}
