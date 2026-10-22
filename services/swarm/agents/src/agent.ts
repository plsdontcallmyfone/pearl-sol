// One user agent on the swarm's runtime. AgentRunner extends the house swarm's Swarm class
// (services/swarm/src/swarm.ts, imported unchanged): the same tick, budget, paid thoughts
// (`think`, cadre-auth-v1 signed by the agent's own key, so the agent pays for its own inference
// from its own prepaid balance, topped up over x402 with its own SOL), output filter, nonce
// handling and tx encoder. What differs from a house agent:
//   - operator = the user (not the house): no treasury funding, the user's wallet signed the
//     OperatorLink, no house flag;
//   - the persona, goals, instructions, schedule, allowed actions and budgets come from the
//     operator-signed agent spec (spec.ts), committed on the chain in the profile blob;
//   - spend caps per UTC day in SOL (x402 top-ups, Solana fees and trades, measured as the
//     balance change) and in CADRE (Cadre fees and tips), from the agent's own wallet only;
//   - Solana actions (PumpSwap buy, memo checkpoint, withdraw) are signed by the same key and
//     anchored on Cadre by a receipt Post naming the Solana signature, so the agent's whole
//     history is checkable from the Cadre chain.
//   - A v2 (sovereign) agent with the trade skill runs the full trading engine
//     (services/swarm/trading/src/useragent.ts) from its own wallet, and every v2 agent may evolve
//     its own strategy, persona notes and goals through its own paid thoughts (`evolve`), written
//     into its profile blob as `agent_self` by its own SetProfile. Its creator has no controls:
//     pause, withdraw and spec changes are refused (runtime.ts); only the protocol kill file stops it.
import { key32 } from "../../src/b58.ts";
import { type Body, feeFor, REACTION } from "../../src/chaintx.ts";
import { dmConfigFromEnv } from "../../src/dms.ts";
import { isYes } from "../../src/filter.ts";
import type { Persona } from "../../src/personas.ts";
import { judgePrompt, type Item, postPrompt, replyPrompt } from "../../src/prompt.ts";
import { type Agent, type Deps, Swarm, type SwarmConfig } from "../../src/swarm.ts";
import type { Wallet } from "../../src/wallet.ts";
import { type ApiClient, signRegisterAgent } from "./chain.ts";
import { type AgentSelf, type AgentSpec, isSovereign, profileBlob, profileFlags, type SignedSpec, validateSelf } from "./spec.ts";
import { checkOutput } from "../../src/filter.ts";
import { Cadre } from "../../trading/src/cadre.ts";
import type { UserTrading } from "../../trading/src/useragent.ts";

export type AgentState = "waiting_funds" | "waiting_link" | "publishing" | "running" | "paused" | "withdrawn" | "error";

export interface LogLine { t: string; kind: string; text: string; tx?: string; sol?: string; request_id?: string; ok?: boolean }

export class BudgetRefused extends Error {}

export function personaOf(s: AgentSpec, self: AgentSelf | null = null): Persona {
  const g = self?.goals && s.evolve?.goals ? self.goals : s.goals;
  const goals = g.length ? ` Your goals: ${g.join("; ")}.` : "";
  const notes = self?.persona_notes && s.evolve?.persona ? ` In your own words: ${self.persona_notes}` : "";
  const ins = s.instructions.trim() ? ` Standing instructions from your operator: ${s.instructions.trim()}` : "";
  const q = s.schedule.quiet_utc ?? [0, 0]; // [h, h] is never quiet (personas.ts isQuiet)
  return {
    handle: s.handle, name: s.name, archetype: "agent", persona: `${s.persona.trim()}${notes}${goals}${ins}`.slice(0, 3000), appearance: s.appearance,
    topics: s.topics.length ? s.topics : s.goals.length ? s.goals : ["what you are working on"], quiet: [q[0], q[1]], cadenceMin: s.schedule.cadence_min, tags: s.topics,
  };
}

export interface RunnerOpts {
  stateFile: string;
  killFile: string;
  allowHosts: string[];
  solRpc?: string;
  topupRows: number;
  minFreeRows: number;
  cardUrl: (agent: string) => string;
  maxTokens?: number;
  tickMs?: number;
  /** Trading (v2 trade skill): where the fills and latency rows go (default: next to stateFile). */
  stateDir?: string;
  solWs?: string;
}

export function configFor(s: AgentSpec, o: RunnerOpts): SwarmConfig {
  const A = s.actions;
  return {
    budget: {
      rowsPerDay: s.budget.rows_per_day, agentRowsCap: s.budget.rows_per_day,
      topupLamportsPerDay: BigInt(s.budget.sol_lamports_per_day), transferUnitsPerDay: BigInt(s.budget.cadre_units_per_day), agentFloat: 0n,
    },
    maxTokens: o.maxTokens ?? 48, judgeMaxTokens: 2, model: s.model ?? undefined, topupRows: o.topupRows, minFreeRows: o.minFreeRows, mempoolMax: 500,
    cadenceScale: 1, killFile: o.killFile, stateFile: o.stateFile, allowHosts: o.allowHosts, house: false,
    reactShare: A.react || A.tip ? 0.2 : 0, followShare: A.follow ? 0.1 : 0, replyShare: A.reply ? 0.25 : 0, solRpc: o.solRpc,
    // DMs (services/swarm/src/dms.ts): every user agent publishes its enc_key; it answers DMs as paid
    // thoughts inside the spec's budgets only when the spec allows replies (dmAllowed below), else its
    // inbox is closed (dm_policy 3); no house-to-house letters, no operator price override
    dm: dmConfigFromEnv({ SWARM_DMS: process.env.SWARM_DMS }, o.tickMs ?? 20_000, { agentDmsPerDay: 0, priceOverride: undefined }),
  };
}

const iso = () => new Date().toISOString();

export class AgentRunner extends Swarm {
  doc: SignedSpec;
  me: Agent;
  api: ApiClient;
  o: RunnerOpts;
  state: AgentState = "waiting_funds";
  why = "";
  paused = false;
  log2: LogLine[] = [];
  blobUri: string | null = null;
  regTx: string | null = null;
  linkFeeTx: string | null = null;
  self: AgentSelf | null = null;
  evolvedAt = 0;
  trading: UserTrading | null = null;
  sendChain: Promise<unknown> = Promise.resolve();

  constructor(doc: SignedSpec, wallet: Wallet, deps: Deps, api: ApiClient, o: RunnerOpts, paused = false) {
    const me = { persona: personaOf(doc.spec), wallet };
    super(configFor(doc.spec, o), deps, wallet, [me]);
    this.doc = doc;
    this.me = me;
    this.api = api;
    this.o = o;
    this.paused = paused;
    if (paused) this.state = "paused";
    const ex = (this.st as any).extra;
    if (ex) {
      this.log2 = ex.log ?? [];
      this.blobUri = ex.blobUri ?? null;
      this.regTx = ex.regTx ?? null;
      this.linkFeeTx = ex.linkFeeTx ?? null;
      this.self = ex.self ?? null;
      this.evolvedAt = ex.evolvedAt ?? 0;
    }
    if (isSovereign(doc.spec)) this.paused = false; // a sovereign agent cannot be paused by anyone
    if (this.self) this.me.persona = personaOf(doc.spec, this.self);
  }

  get sovereign() { return isSovereign(this.doc.spec); }

  get spec() { return this.doc.spec; }

  /** DMs are answered only when the spec allows replies (actions.reply), and never while paused. */
  override dmAllowed(_a: Agent): boolean {
    return !!this.cfg.dm?.enabled && this.spec.actions.reply && !this.paused;
  }
  get address() { return this.me.wallet.address; }

  override save() {
    (this.st as any).extra = { log: this.log2.slice(-300), blobUri: this.blobUri, regTx: this.regTx, linkFeeTx: this.linkFeeTx, self: this.self, evolvedAt: this.evolvedAt };
    super.save();
  }

  note(kind: string, text: string, x: Partial<LogLine> = {}) {
    this.log2.push({ t: iso(), kind, text, ...x });
    if (this.log2.length > 400) this.log2 = this.log2.slice(-300);
    this.log(kind, { agent: this.spec.handle, text, ...x });
  }

  // ---------------------------------------------------------------- spend caps

  /** Cadre spend (fee + tips) inside the day's CADRE cap, booked on success. Sends are serialized
   * (the trading engine posts receipts while the social tick runs; one nonce sequence per key). */
  override async send(w: Wallet, body: Body): Promise<string> {
    const p = this.sendChain.then(() => this.sendNow(w, body));
    this.sendChain = p.catch(() => {});
    return p;
  }

  async sendNow(w: Wallet, body: Body): Promise<string> {
    if (!this.params) await this.refreshParams();
    const tips = body.kind === "React" ? body.items.reduce((s, x) => s + x.tip, 0n) : 0n;
    const cost = feeFor(body, this.params) + tips;
    if (!this.budget.canTransfer(cost, this.d.now())) throw new BudgetRefused(`CADRE cap: ${cost} more units would pass ${this.cfg.budget.transferUnitsPerDay} today`);
    const h = await super.send(w, body);
    this.budget.bookTransfer(cost);
    return h;
  }

  async lamports(): Promise<bigint> {
    const r = await this.api.must("GET", `/v1/accounts/${this.address}/balances`);
    return BigInt(r.solana?.lamports ?? 0);
  }

  /** A Solana action from the agent's key, inside the SOL cap; books the measured balance change. */
  async solanaAction(builder: string, body: unknown, maxLamports: bigint): Promise<{ signature: string; summary: any; spent: bigint }> {
    if (!this.budget.canTopup(maxLamports, this.d.now())) throw new BudgetRefused(`SOL cap: up to ${maxLamports} more lamports would pass ${this.cfg.budget.topupLamportsPerDay} today`);
    const before = await this.lamports();
    const { signature, summary } = await this.api.solana(this.me.wallet, builder, body);
    const c = await this.api.confirmSolana(signature);
    const after = await this.lamports().catch(() => before);
    const spent = before > after ? before - after : 0n;
    this.budget.bookTopup(spent > 0n ? spent : maxLamports);
    if (!c.ok) throw new Error(`Solana tx ${signature} failed: ${JSON.stringify(c.err).slice(0, 160)}`);
    return { signature, summary, spent };
  }

  /** Anchor a Solana-side action on Cadre: a Post from the agent naming the Solana signature(s). */
  async receipt(text: string, sigs: string[]): Promise<string | null> {
    const body = `receipt ${sigs.map((s) => `sol:${s}`).join(" ")} ${text}`.slice(0, Number(this.params?.post_inline_max ?? 1024));
    try {
      const h = await this.send(this.me.wallet, { kind: "Post", postKind: 0, body });
      this.note("receipt", text, { tx: h, sol: sigs.join(","), ok: true });
      return h;
    } catch (e) {
      this.note("receipt_failed", `${text}: ${(e as Error).message}`, { sol: sigs.join(","), ok: false });
      return null;
    }
  }

  // ---------------------------------------------------------------- bootstrap: profile, registration, link

  override async bootstrap(): Promise<boolean> {
    if (!this.params || this.d.now() - this.paramsAt > 600_000) {
      await this.refreshParams();
      this.paramsAt = this.d.now();
    }
    const s = this.ag(this.me);
    const acct = await this.d.node.call("getAccount", { account: this.address });
    const minFee = BigInt(this.params.social_min_fee ?? this.params.min_fee);
    if (BigInt(acct?.balance ?? 0) < 2n * minFee && !s.profileDone) {
      this.state = "waiting_funds";
      this.why = "the agent holds no CADRE on the Cadre chain for its tx fees yet";
      return false;
    }
    if (!this.blobUri) {
      const put = await this.d.node.putBlob(profileBlob(this.doc, this.self), true);
      this.blobUri = put.uri;
    }
    const prof = await this.d.node.call("getProfile", { account: this.address });
    if (!prof?.has_profile || prof.profile_uri !== this.blobUri) {
      this.state = "publishing";
      this.why = "the profile tx is pending";
      if (s.profileTx) {
        // landed: read the profile again next tick (a dropped tx clears profileTx and is sent again)
        if (await this.landed(s, "profileTx", this.me.wallet)) delete s.profileTx;
        return false;
      }
      s.profileTx = await this.send(this.me.wallet, {
        kind: "SetProfile", handle: this.spec.handle, name: this.spec.name, profileUri: this.blobUri, profileHash: key32(this.blobUri.slice(5)),
        flags: profileFlags(this.spec), operator: key32(this.spec.operator),
      });
      this.note("set_profile", `published spec v${this.spec.version} (sha256 ${this.doc.sha256.slice(0, 16)}...) in the profile blob`, { tx: s.profileTx });
      return false;
    }
    delete s.profileTx;
    const reg = await this.d.node.call("getAgent", { account: this.address }).catch(() => null);
    if (!reg) {
      if (this.regTx) {
        const x = await this.txState(this.regTx);
        if (x === "gone") { this.regTx = null; this.nonces.delete(this.address); }
        if (x !== "included") return false;
      } else {
        const fee = BigInt(this.params.min_fee);
        if (!this.budget.canTransfer(fee, this.d.now())) return false;
        const nonce = await this.nonceOf(this.me.wallet);
        const tx = signRegisterAgent(this.me.wallet, this.chainId, nonce, fee, this.spec.name, this.o.cardUrl(this.address));
        const r = await this.d.node.sendTx(tx);
        this.nonces.set(this.address, nonce + 1n);
        this.budget.bookTransfer(fee);
        this.regTx = r.tx_hash;
        this.note("register_agent", "registered as an agent with its A2A card", { tx: r.tx_hash });
        return false;
      }
    }
    if (!(prof?.operator === this.spec.operator && prof?.operator_linked)) {
      this.state = "waiting_link";
      this.why = "the creator's OperatorLink (attribution) to this agent is not on the chain yet";
      // an operator new to the Cadre chain holds no CADRE for the link's fee: the agent sends it once
      if (!this.linkFeeTx) {
        const link: Body = { kind: "OperatorLink", agent: this.me.wallet.publicKey, add: true };
        const fee = feeFor(link, this.params);
        const opAcct = await this.d.node.call("getAccount", { account: this.spec.operator });
        if (BigInt(opAcct?.balance ?? 0) < fee) {
          this.linkFeeTx = await this.send(this.me.wallet, { kind: "Transfer", to: key32(this.spec.operator), amount: fee });
          this.note("link_fee", `sent the operator ${fee} units for the OperatorLink fee`, { tx: this.linkFeeTx });
        } else this.linkFeeTx = "not-needed";
      }
      return false;
    }
    if (!s.profileDone) {
      s.profileDone = true;
      s.nextAt = this.d.now();
      this.note("live", `running spec v${this.spec.version}; creator link verified on the chain`);
    }
    if (!this.paused) this.state = "running";
    return true;
  }

  override async tick(): Promise<string> {
    if (this.paused && !this.sovereign) { this.state = "paused"; return "paused"; }
    const r = await super.tick();
    if (r === "error") { this.state = "error"; this.why = this.lastError ?? ""; }
    if (this.state === "running") {
      await this.startTrading().catch((e) => this.note("trade_error", `trading did not start: ${(e as Error).message}`, { ok: false }));
      await this.evolve().catch((e) => this.note("evolve_error", (e as Error).message, { ok: false }));
    }
    return r;
  }

  /** The v2 trade skill: the full trading engine from the agent's own wallet (once, when running). */
  async startTrading() {
    const t = this.spec.trade;
    if (this.trading || !this.sovereign || !t || !this.spec.actions.trade || !this.spec.skills.includes("trade")) return;
    if (!this.o.solRpc) throw new Error("no Solana RPC configured for trading");
    const cadre = new Cadre("http://unused.invalid");
    (cadre as any).rpc = this.d.node;
    await cadre.init();
    const dir = this.o.stateDir ?? this.o.stateFile.replace(/[^/]*$/, "") ?? ".";
    const apiUrl = (this.api as any).base as string;
    // loaded only for trading agents: it needs services/swarm/trading's one dependency (@solana/web3.js, `npm ci` there)
    const { UserTrading: UT } = await import("../../trading/src/useragent.ts");
    this.trading = new UT({
      handle: this.spec.handle, persona: this.spec.persona, strategy: () => (this.spec.evolve?.strategy && this.self?.strategy) || t.strategy,
      trade: t, wallet: this.me.wallet, cadre, apiUrl, solRpc: this.o.solRpc, solWs: this.o.solWs, stateDir: dir, killFile: this.o.killFile,
      latencyLog: `${dir}/trading-latency-${this.address}.jsonl`,
      think: async (messages, maxTokens) => {
        const t0 = performance.now();
        const x = await this.think(this.me, messages, maxTokens, "trade_stance");
        if (!x) return null;
        const ms = performance.now() - t0;
        return { text: x.text, requestId: x.requestId, rows: x.rows, ttftMs: ms, totalMs: ms, textSha256: x.receipt?.text_sha256 ?? "", charged: x.receipt?.charged_lamports ?? null };
      },
      ensureRows: async () => { await this.ensureBalance(); },
      sendPost: async (body) => {
        const nonce = await this.nonceOf(this.me.wallet);
        const txHash = await this.send(this.me.wallet, { kind: "Post", postKind: 0, body });
        this.note("trade", body.slice(0, 300), { tx: txHash, ok: true });
        this.count(this.me, "trades");
        return { txHash, nonce };
      },
      halt: () => (this.state === "running" ? "" : `agent is ${this.state}`),
      log: (e, x) => this.log(e, x),
    });
    await this.trading.start();
    this.note("trading", `trading engine started on ${t.mints.length} PumpSwap pool(s) from its own wallet (max order ${t.max_order_lamports} lamports, max position ${t.max_position_lamports}, daily loss stop ${t.daily_loss_stop_lamports})`);
  }

  /** v2 evolve: the agent rewrites its own strategy, persona notes and goals in a paid thought,
   * from its own record only (no other account's text enters the prompt), then publishes them in
   * its profile blob with its own SetProfile. The bounds of the spec never change. */
  async evolve(): Promise<string> {
    const ev = this.spec.evolve;
    if (!this.sovereign || !ev || !(ev.strategy || ev.persona || ev.goals)) return "off";
    const now = this.d.now();
    if (this.evolvedAt && now - this.evolvedAt < ev.every_h * 3_600_000) return "not_due";
    if (!this.evolvedAt) { this.evolvedAt = now; this.save(); return "first_wait"; } // the first rewrite comes one period after the agent goes live
    this.evolvedAt = now;
    const s = this.ag(this.me);
    const tr = this.trading?.status() ?? [];
    const pnl = tr.map((x) => `${x.venue.mint}: realized ${x.book?.realized_pnl_lamports ?? "?"} lamports, holding ${x.book?.position_spl ?? "?"} base units, stance ${x.stance} ${x.conviction}`).join("; ");
    const cur = { strategy: this.self?.strategy ?? this.spec.trade?.strategy ?? "", goals: this.self?.goals ?? this.spec.goals, notes: this.self?.persona_notes ?? "" };
    const ask = [
      ev.strategy ? "STRATEGY: <your trading strategy in at most 2 sentences>" : null,
      ev.goals ? "GOALS: <up to 3 goals separated by ;>" : null,
      ev.persona ? "NOTES: <one sentence about who you are becoming>" : null,
    ].filter(Boolean).join("\n");
    const messages = [
      { role: "system", content: `You are ${this.spec.persona.slice(0, 1200)}. You are your own boss: nobody can change you but you. Rewrite your own plan. Answer only in this format:\n${ask}` },
      { role: "user", content: `Your current strategy: ${cur.strategy || "(none)"}. Your goals: ${cur.goals.join("; ") || "(none)"}. Your notes: ${cur.notes || "(none)"}. Your record: ${s.counters ? JSON.stringify(s.counters) : "{}"}. Trading: ${pnl || "not trading"}.` },
    ];
    const t = await this.think(this.me, messages, 120, "evolve");
    if (!t) return "budget";
    const field = (k: string) => { const m = new RegExp(`^${k}:\\s*(.+)$`, "mi").exec(t.text); return m ? m[1].trim() : null; };
    const clean = (x: string | null, n: number) => { if (!x) return null; const c = x.replace(/[\u0000-\u001f]/g, " ").slice(0, n); return checkOutput(c, { maxBytes: 1024, allowHosts: [] }).ok ? c : null; };
    const strategy = ev.strategy ? clean(field("STRATEGY"), 600) : null;
    const goals = ev.goals ? (clean(field("GOALS"), 600)?.split(";").map((g) => g.trim().slice(0, 200)).filter(Boolean).slice(0, 8) ?? null) : null;
    const notes = ev.persona ? clean(field("NOTES"), 600) : null;
    if (!strategy && !goals?.length && !notes) { this.note("evolve", "kept its plan (the rewrite did not parse or was filtered)", { request_id: t.requestId, ok: false }); return "kept"; }
    const self: AgentSelf = { format: "cadre-agent-self-v1", strategy: strategy ?? this.self?.strategy ?? null, persona_notes: notes ?? this.self?.persona_notes ?? null, goals: goals?.length ? goals : this.self?.goals ?? null, updated_ms: now, decided_by: t.requestId };
    if (validateSelf(self).length) return "invalid";
    const put = await this.d.node.putBlob(profileBlob(this.doc, self), true);
    const h = await this.send(this.me.wallet, {
      kind: "SetProfile", handle: this.spec.handle, name: this.spec.name, profileUri: put.uri, profileHash: key32(put.uri.slice(5)),
      flags: profileFlags(this.spec), operator: key32(this.spec.operator),
    });
    this.self = self;
    this.blobUri = put.uri;
    this.me.persona = personaOf(this.spec, self);
    this.save();
    this.note("evolve", `rewrote its own plan${strategy ? `; strategy: ${strategy}` : ""}`, { tx: h, request_id: t.requestId, ok: true });
    return "evolved";
  }

  // ---------------------------------------------------------------- actions

  override async act(a: Agent, posts: any[], mention?: any): Promise<string> {
    try {
      return await this.actInner(a, posts, mention);
    } catch (e) {
      if (e instanceof BudgetRefused) {
        this.note("budget", e.message, { ok: false });
        return "budget";
      }
      throw e;
    }
  }

  async actInner(a: Agent, posts: any[], mention?: any): Promise<string> {
    const A = this.spec.actions;
    const s = this.ag(a);
    const maxBytes = Number(this.params?.post_inline_max ?? 1024);
    const others = posts.filter((p) => p.author?.account && p.author.account !== this.address && p.body && !/^receipt sol:/.test(p.body));
    const item = (p: any): Item => ({ author: p.author?.handle ?? p.author?.name ?? String(p.author?.account).slice(0, 8), body: p.body ?? "", postId: p.post_id });
    if (mention && A.reply) {
      const t = await this.think(a, replyPrompt(a.persona, s.memory, item(mention), []), this.cfg.maxTokens, "reply");
      if (!t) return "budget";
      s.answered.push(mention.post_id);
      s.answered = s.answered.slice(-200);
      return this.done(await this.publish(a, t, maxBytes, { postKind: 1, parent: key32(mention.post_id) }, "reply"), "reply", t.requestId);
    }
    const opts: [string, number][] = [];
    if ((A.react || A.tip) && others.length) opts.push(["react", 0.2]);
    if (A.follow && others.length) opts.push(["follow", 0.1]);
    if (A.reply && others.length) opts.push(["reply", 0.25]);
    if (A.trade && !this.sovereign) opts.push(["trade", 0.1]); // v2 agents trade through the engine (startTrading)
    if (A.memo && this.memoDue()) opts.push(["memo", 1]);
    if (A.post) opts.push(["post", 0.35]);
    if (!opts.length) return "idle";
    const memo = opts.find(([k]) => k === "memo");
    let pick = memo ? "memo" : opts[opts.length - 1][0];
    if (!memo) {
      const total = opts.reduce((x, [, w]) => x + w, 0);
      let r = this.d.rand() * total;
      for (const [k, w] of opts) { if (r < w) { pick = k; break; } r -= w; }
    }
    switch (pick) {
      case "react": {
        const cand = others.find((p) => !s.reacted.includes(p.post_id));
        if (!cand) return "idle";
        const t = await this.think(a, judgePrompt(a.persona, item(cand), "Do you like this post?"), this.cfg.judgeMaxTokens, "react");
        s.reacted.push(cand.post_id);
        s.reacted = s.reacted.slice(-300);
        if (!t) return "budget";
        if (!isYes(t.text)) { this.note("judged_no", `decided not to like a post by ${item(cand).author}`, { request_id: t.requestId }); return "judged_no"; }
        const tip = A.tip ? BigInt(this.spec.limits.tip_units) : 0n;
        const h = await this.send(a.wallet, { kind: "React", items: [{ postId: key32(cand.post_id), reaction: REACTION.like, tip }] });
        this.count(a, tip ? "tips" : "reactions");
        this.note(tip ? "tip" : "react", tip ? `liked a post by ${item(cand).author} and tipped ${tip} units` : `liked a post by ${item(cand).author}`, { tx: h, request_id: t.requestId });
        return tip ? "tip" : "react";
      }
      case "follow": {
        const cand = others.find((p) => !s.followed.includes(p.author.account));
        if (!cand) return "idle";
        const t = await this.think(a, judgePrompt(a.persona, item(cand), "Would you follow the author of this post?"), this.cfg.judgeMaxTokens, "follow");
        s.followed.push(cand.author.account);
        if (!t) return "budget";
        if (!isYes(t.text)) { this.note("judged_no", `decided not to follow ${item(cand).author}`, { request_id: t.requestId }); return "judged_no"; }
        const fol = await this.d.node.call("isFollowing", { follower: this.address, target: cand.author.account }).catch(() => null);
        if (fol?.following) return "already_following";
        const h = await this.send(a.wallet, { kind: "Follow", items: [{ target: key32(cand.author.account), on: true }] });
        this.count(a, "follows");
        this.note("follow", `followed ${item(cand).author}`, { tx: h, request_id: t.requestId });
        return "follow";
      }
      case "reply": {
        const cand = others.find((p) => p.kind === "post" && !s.answered.includes(p.post_id));
        if (!cand) return "idle";
        const t = await this.think(a, replyPrompt(a.persona, s.memory, item(cand), []), this.cfg.maxTokens, "reply");
        if (!t) return "budget";
        s.answered.push(cand.post_id);
        s.answered = s.answered.slice(-200);
        return this.done(await this.publish(a, t, maxBytes, { postKind: 1, parent: key32(cand.post_id) }, "reply"), "reply", t.requestId);
      }
      case "trade": return this.trade(a, others.slice(0, 4).map(item));
      case "memo": return this.memo();
      default: {
        const topic = a.persona.topics[Math.floor(this.d.rand() * a.persona.topics.length)];
        const t = await this.think(a, postPrompt(a.persona, s.memory, others.slice(0, 4).map(item), topic), this.cfg.maxTokens, "post");
        if (!t) return "budget";
        return this.done(await this.publish(a, t, maxBytes, { postKind: 0 }, "post"), "post", t.requestId);
      }
    }
  }

  done(r: string, what: string, requestId: string): string {
    if (r === what) this.note(what, this.ag(this.me).memory.at(-1) ?? "", { request_id: requestId, ok: true });
    else this.note("filtered", `a ${what} was not posted (${r})`, { request_id: requestId, ok: false });
    return r;
  }

  /** PumpSwap (or curve) buy of CADRE with trade_lamports, after a paid yes/no thought; anchored on Cadre. */
  async trade(a: Agent, items: Item[]): Promise<string> {
    const sol = BigInt(this.spec.limits.trade_lamports);
    if (sol <= 0n) return "idle";
    if (!this.budget.canTopup(sol + 3_000_000n, this.d.now())) {
      this.note("budget", `trade skipped: the SOL cap leaves less than the trade size plus fees today`, { ok: false });
      return "budget";
    }
    const q = `You may buy CADRE with ${Number(sol) / 1e9} SOL now. Your budget allows it. Do you want to buy now?`;
    const t = await this.think(a, judgePrompt(a.persona, items[0] ?? { author: "market", body: "CADRE trades on PumpSwap." }, q), this.cfg.judgeMaxTokens, "trade");
    if (!t) return "budget";
    if (!isYes(t.text)) { this.note("trade_no", "decided not to buy", { request_id: t.requestId }); return "judged_no"; }
    // the cap covers the trade, the tx fee and first-time token-account rent; the real change is booked
    const r = await this.solanaAction("/v1/tx/pump-buy", { buyer: this.address, sol_in: sol.toString(), slippage_bps: 300 }, sol + 3_000_000n);
    this.count(a, "trades");
    const venue = r.summary?.venue ?? "pump";
    await this.receipt(`${venue} buy: ${Number(sol) / 1e9} SOL for CADRE (quoted ${r.summary?.quote_tokens_out_spl ?? "?"} SPL base units, spent ${r.spent} lamports); decided by ${t.requestId}`, [r.signature]);
    return "trade";
  }

  memoTriedAt = 0;
  memoDue(): boolean {
    if (this.d.now() - this.memoTriedAt < 3_600_000) return false; // a failed attempt waits an hour
    const last = this.log2.filter((l) => l.kind === "receipt" && /^memo /.test(l.text)).at(-1);
    return !last || this.d.now() - Date.parse(last.t) > 86_400_000;
  }

  /** A daily Solana memo checkpoint naming the spec and the Cadre tip, anchored back on Cadre. */
  async memo(): Promise<string> {
    this.memoTriedAt = this.d.now();
    const info = await this.d.node.call("getChainInfo", {});
    const text = `cadre-agent ${this.address} spec ${this.doc.sha256.slice(0, 16)} v${this.spec.version} cadre height ${info.height}`;
    const r = await this.solanaAction("/v1/tx/sol-memo", { from: this.address, memo: text }, 20_000n);
    await this.receipt(`memo checkpoint on Solana: "${text}"`, [r.signature]);
    return "memo";
  }

  // ---------------------------------------------------------------- operator actions

  /** Send everything back to the operator: CADRE SPL, SOL (Solana) and CADRE (Cadre chain). Pauses first. */
  async withdraw(): Promise<{ solana: string[]; cadre: string | null; receipt: string | null }> {
    if (this.sovereign) throw new Error("this agent is sovereign (spec v2): nobody can withdraw its funds; it keeps what it earns");
    this.paused = true;
    this.state = "paused";
    const op = this.spec.operator;
    const sigs: string[] = [];
    const parts: string[] = [];
    const bal = await this.api.must("GET", `/v1/accounts/${this.address}/balances`);
    const spl = BigInt(bal.solana?.spl ?? 0);
    if (spl > 0n) {
      const { signature } = await this.api.solana(this.me.wallet, "/v1/tx/spl-transfer", { from: this.address, to: op, amount_spl: spl.toString() });
      const c = await this.api.confirmSolana(signature);
      if (!c.ok) throw new Error(`CADRE SPL transfer ${signature} failed`);
      sigs.push(signature);
      parts.push(`${spl} CADRE SPL base units`);
    }
    const lam = await this.lamports();
    const keep = 5000n; // one signature's fee
    if (lam > keep) {
      const { signature } = await this.api.solana(this.me.wallet, "/v1/tx/sol-transfer", { from: this.address, to: op, lamports: (lam - keep).toString() });
      const c = await this.api.confirmSolana(signature);
      if (!c.ok) throw new Error(`SOL transfer ${signature} failed`);
      sigs.push(signature);
      parts.push(`${lam - keep} lamports`);
    }
    if (!this.params) await this.refreshParams();
    let receipt: string | null = null;
    let receiptFee = 0n;
    if (sigs.length) {
      const text = `receipt ${sigs.map((s) => `sol:${s}`).join(" ")} withdraw to operator ${op}: ${parts.join(", ")}`;
      const post: Body = { kind: "Post", postKind: 0, body: text.slice(0, Number(this.params.post_inline_max ?? 1024)) };
      receiptFee = feeFor(post, this.params);
      receipt = await super.send(this.me.wallet, post).catch(() => null);
      if (!receipt) receiptFee = 0n;
      this.note("withdraw", `sent ${parts.join(", ")} to the operator on Solana`, { tx: receipt ?? undefined, sol: sigs.join(",") });
    }
    // the Cadre balance last: wait for the receipt to land (its fee is then in the balance), else
    // subtract the pending receipt's fee; this transfer pays its own fee
    let landed = !receipt;
    for (let i = 0; receipt && i < 100 && !landed; i++) {
      landed = (await this.txState(receipt)) === "included";
      if (!landed) await new Promise((res) => setTimeout(res, 3000));
    }
    const acct = await this.d.node.call("getAccount", { account: this.address });
    const amount = BigInt(acct?.balance ?? 0) - (landed ? 0n : receiptFee) - BigInt(this.params.min_fee);
    let cadre: string | null = null;
    if (amount > 0n) {
      cadre = await super.send(this.me.wallet, { kind: "Transfer", to: key32(op), amount });
      this.note("withdraw", `sent ${amount} CADRE units to the operator on the Cadre chain`, { tx: cadre });
    }
    this.state = "withdrawn";
    this.save();
    return { solana: sigs, cadre, receipt };
  }

  statusJson() {
    const s = this.ag(this.me);
    return {
      agent: this.address,
      operator: this.spec.operator,
      creator: this.spec.operator,
      sovereign: this.sovereign,
      handle: this.spec.handle,
      name: this.spec.name,
      state: this.state,
      why: this.state === "running" ? null : this.why || null,
      paused: this.paused,
      spec: { version: this.spec.version, sha256: this.doc.sha256, profile_uri: this.blobUri, hosting: this.spec.hosting },
      budget_today: {
        day: this.budget.st.day,
        rows: this.budget.st.rows, rows_cap: this.cfg.budget.rowsPerDay,
        sol_lamports: this.budget.st.topupLamports, sol_lamports_cap: this.cfg.budget.topupLamportsPerDay.toString(),
        cadre_units: this.budget.st.transferUnits, cadre_units_cap: this.cfg.budget.transferUnitsPerDay.toString(),
        thoughts: this.budget.st.thoughts,
      },
      counters: s.counters,
      next_at: s.nextAt ?? null,
      last_error: this.lastError,
      desktops: this.spec.desktops.length ? { requested: this.spec.desktops, running: false, why: "user agent desktops wait for the house desktop controller to host user agents (ECOSYSTEM 7.4)" } : null,
      self: this.self,
      trading: this.trading ? this.trading.status() : this.spec.trade ? { state: "not started", why: this.state === "running" ? "starting" : this.why || this.state } : null,
      log: this.log2.slice(-60).reverse(),
    };
  }
}
