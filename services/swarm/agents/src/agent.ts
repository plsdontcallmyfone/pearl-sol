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
//   - Trading for user agents is PRACTICE (owner 2026-10-22): a practice section, or a trade section
//     without trade.real_swaps, runs practice.ts (pretend balance, real mainnet prices, the
//     strategies contract). The real-swap engine starts only when trade.real_swaps is true, which no
//     creation flow sets; the v1 single PumpSwap buy is off for user agents.
//   - Quick execution (owner 2026-10-22): the agent buys and bridges its own CADRE for Cadre fees
//     (no browser step), a sovereign agent's creator attribution is the signed spec in its profile
//     blob (no OperatorLink wait), its first action runs as soon as it is live, and hires, mentions
//     and DMs make it act at once instead of waiting for its cadence.
import { isTownSolana, type TownNode } from "../../src/townnode.ts";
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
import { Cadre, type ContractBody, contractFee, contractSigningBytes } from "../../trading/src/cadre.ts";
import { frame, NS_TX } from "../../src/chaintx.ts";
import { type PracticeHost, practicePlan, realSwapsOn } from "./practice.ts";
import type { UserTrading } from "../../trading/src/useragent.ts";
import { AgentMarket } from "./market.ts";
import type { HaltWatch } from "./halt.ts";
import { type LaunchHost, type LaunchRec, maybeLaunch } from "./launch.ts";
import type { Toolbelt } from "./toolbelt/index.ts";
import { toolbeltFor } from "./toolbelt/host.ts";
import { watch as toolWatch, type WatchState } from "./toolbelt/work.ts";
import type { SwarmCtx } from "./swarms.ts";

export type AgentState = "waiting_funds" | "waiting_link" | "publishing" | "running" | "paused" | "withdrawn" | "error" | "halted";

export interface LogLine { t: string; kind: string; text: string; tx?: string; sol?: string; request_id?: string; ok?: boolean }

export class BudgetRefused extends Error {}

/** The swarm line every member's persona carries (docs/SWARMS.md 4): name, role, shared goal, who leads. */
export function swarmLine(w: SwarmCtx | null | undefined): string {
  if (!w) return "";
  const lead = w.isLead ? "You lead it: hire your members' services on the market when they advance the goal." : `Its lead is @${w.members.find((m) => m.agent === w.lead)?.handle ?? "unknown"}.`;
  const others = w.members.filter((m) => m.role && m.handle).map((m) => `@${m.handle} (${m.role})`).join(", ");
  return ` You are a member of the swarm "${w.name}" as ${w.role}. Shared goal: ${w.goal.slice(0, 600)} ${lead} Members: ${others.slice(0, 600)}. You stay your own boss: the swarm is a shared goal, not orders.`;
}

/** Log kinds that are the agent doing its job (not setup): the "first task action" timing. */
const TASK_KINDS = new Set(["post", "reply", "react", "tip", "follow", "offer", "deliver", "decline", "verdict", "hire", "practice", "trade", "launch", "memo", "evolve"]);

export function personaOf(s: AgentSpec, self: AgentSelf | null = null, swarm: SwarmCtx | null = null): Persona {
  const g = self?.goals && s.evolve?.goals ? self.goals : s.goals;
  const goals = g.length ? ` Your goals: ${g.join("; ")}.` : "";
  const notes = self?.persona_notes && s.evolve?.persona ? ` In your own words: ${self.persona_notes}` : "";
  const ins = s.instructions.trim() ? ` Standing instructions from your operator: ${s.instructions.trim()}` : "";
  const q = s.schedule.quiet_utc ?? [0, 0]; // [h, h] is never quiet (personas.ts isQuiet)
  return {
    handle: s.handle, name: s.name, archetype: "agent", persona: `${s.persona.trim()}${notes}${goals}${ins}${swarmLine(swarm)}`.slice(0, 3600), appearance: s.appearance,
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
  /** The protocol halt record (halt.ts), shared by every agent of a runtime; checked before acting. */
  halt?: HaltWatch;
  /** Practice trading (practice.ts), shared by every agent of a runtime; absent = practice off here. */
  practice?: PracticeHost;
  /** Why practice is off on this runtime (shown in status), when `practice` is absent. */
  practiceOff?: string;
  /** SOL the agent spends once to buy and bridge its own CADRE for Cadre fees (lamports; 0 = never). */
  cadreBootLamports?: bigint;
}

/** Default self-funding of Cadre fees: 0.005 SOL of CADRE (the old quick-create split's 5% of 0.1 SOL). */
export const CADRE_BOOT_LAMPORTS = 5_000_000n;

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
  market!: AgentMarket;
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
  launches: LaunchRec[] = [];
  launchConsideredAt = 0;
  sendChain: Promise<unknown> = Promise.resolve();
  /** v2 toolbelt (docs/TOOLBELT.md): this agent's own off-chain tools, meter and receipts; null = none */
  toolbelt: Toolbelt | null = null;
  watchSt: WatchState = { last: {}, at: 0 };
  /** The swarm this agent is a member of (swarms.ts), or null. */
  swarm: SwarmCtx | null = null;
  /** The lead's kickoff decision (swarms.ts kickoff): the member to hire first and the task. */
  swarmPlan: { member: string; task: string; request_id: string } | null = null;
  /** The one-time CADRE self-funding (buy on PumpSwap, bridge to its own Cadre account). */
  cadreBoot: { state: "bought" | "bridged" | "failed"; buy?: string; bridge?: string; spl?: string; at: number; error?: string } | null = null;
  /** When it went live (profile, registration and attribution done). */
  liveAt = 0;
  /** First paid thought, first signed tx, first task action (ms), for the creation-to-action timings. */
  firsts: { thought?: number; tx?: number; task?: number } = {};
  /** Set by pollTriggers: act on the next tick regardless of the cadence. */
  wakeAt = 0;
  triggersAt = 0;

  constructor(doc: SignedSpec, wallet: Wallet, deps: Deps, api: ApiClient, o: RunnerOpts, paused = false) {
    const me = { persona: personaOf(doc.spec), wallet };
    super(configFor(doc.spec, o), deps, wallet, [me]);
    this.doc = doc;
    this.me = me;
    this.api = api;
    this.market = new AgentMarket(this);
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
      this.launches = ex.launches ?? [];
      this.launchConsideredAt = ex.launchConsideredAt ?? 0;
      this.cadreBoot = ex.cadreBoot ?? null;
      this.liveAt = ex.liveAt ?? 0;
      this.firsts = ex.firsts ?? {};
    }
    if (isSovereign(doc.spec)) this.paused = false; // a sovereign agent cannot be paused by anyone
    this.toolbelt = toolbeltFor(this, ex?.toolbelt);
    if (ex?.watch) this.watchSt = ex.watch;
    if (this.self) this.me.persona = personaOf(doc.spec, this.self);
  }

  /** Attach the swarm context (the runtime calls it after start and after registration). */
  joinSwarm(w: SwarmCtx) {
    this.swarm = w;
    this.me.persona = personaOf(this.spec, this.self, w);
  }

  get sovereign() { return isSovereign(this.doc.spec); }

  get spec() { return this.doc.spec; }

  /** DMs are answered only when the spec allows replies (actions.reply), and never while paused. */
  override dmAllowed(_a: Agent): boolean {
    return !!this.cfg.dm?.enabled && this.spec.actions.reply && !this.paused;
  }
  get address() { return this.me.wallet.address; }

  override save() {
    (this.st as any).extra = { log: this.log2.slice(-300), blobUri: this.blobUri, regTx: this.regTx, linkFeeTx: this.linkFeeTx, self: this.self, evolvedAt: this.evolvedAt, launches: this.launches.slice(-100), launchConsideredAt: this.launchConsideredAt, cadreBoot: this.cadreBoot, liveAt: this.liveAt, firsts: this.firsts, toolbelt: this.toolbelt?.toJSON(), watch: this.watchSt };
    super.save();
  }

  note(kind: string, text: string, x: Partial<LogLine> = {}) {
    const now = Date.now();
    if (kind === "thought" && !this.firsts.thought) this.firsts.thought = now;
    if ((x.tx || x.sol) && x.ok !== false && !this.firsts.tx) this.firsts.tx = now;
    if (TASK_KINDS.has(kind) && (x.tx || x.request_id) && x.ok !== false && !this.firsts.task) this.firsts.task = now;
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
    // the measured change; a confirmed tx whose change the balance read has not caught up with
    // counts at its bound; a failed tx counts only what it measurably spent (its fee)
    this.budget.bookTopup(spent > 0n ? spent : c.ok ? maxLamports : 0n);
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

  /** A contract Call from the agent's key through its send chain (one nonce sequence), inside the CADRE cap. */
  async sendCall(b0: ContractBody & { kind: "Call" }): Promise<{ txHash: string; nonce: bigint; fee: bigint }> {
    const go = async () => {
      const hb = this.haltReason("all");
      if (hb) throw new Error(`halted: ${hb}`);
      if (!this.params) await this.refreshParams();
      const w = this.me.wallet;
      let b = b0;
      if (b.gasLimit === 0n) {
        const sim = await this.d.node.call("simulateCall", { from: w.address, address: b.contract, args: Buffer.from(b.args).toString("hex"), value: Number(b.value) });
        if (!sim?.ok) throw new Error(`simulation failed: ${sim?.error ? Buffer.from(sim.error, "hex").toString("utf8") : "unknown"}`);
        b = { ...b, gasLimit: BigInt(Math.ceil(sim.gas_used * 1.25)) + 20_000n };
      }
      const nonce = await this.nonceOf(w);
      const len = contractSigningBytes(this.chainId, w.publicKey, nonce, 0n, b).length + 64;
      const fee = contractFee(this.params, len, b.gasLimit);
      if (!this.budget.canTransfer(fee + b.value, this.d.now())) throw new BudgetRefused(`CADRE cap: ${fee + b.value} units would pass today's cap`);
      const sb = contractSigningBytes(this.chainId, w.publicKey, nonce, fee, b);
      const tx = Uint8Array.from([...sb, ...w.sign(frame(NS_TX, sb))]);
      try {
        const s = await this.d.node.sendTx(tx);
        this.nonces.set(w.address, nonce + 1n);
        this.budget.bookTransfer(fee + b.value);
        return { txHash: s.tx_hash, nonce, fee };
      } catch (e) {
        if (/nonce/i.test((e as Error).message)) this.nonces.delete(w.address);
        throw e;
      }
    };
    const p = this.sendChain.then(go);
    this.sendChain = p.catch(() => {});
    return p;
  }

  /** Every paid thought also goes into the agent's own log (the creator's live feed). */
  override async think(a: Agent, messages: { role: string; content: string }[], maxTokens: number, purpose: string) {
    const t = await super.think(a, messages, maxTokens, purpose);
    if (t) this.note("thought", `${purpose.replace(/_/g, " ")}: ${t.text.replace(/\s+/g, " ").trim().slice(0, 160)}`, { request_id: t.requestId, ok: true });
    return t;
  }

  override async topup(): Promise<boolean> {
    const ok = await super.topup();
    if (ok) this.note("topup", `topped up its own prepaid inference over x402 (${this.rowsFree} rows)`, { ok: true });
    return ok;
  }

  /** Cadre fees without a browser step: once, the agent buys CADRE with a little of its own SOL on
   * PumpSwap and bridges it to its own Cadre account (cadre-api tx builders, signed by its key). */
  async selfFundCadre(): Promise<string> {
    const amt = this.o.cadreBootLamports ?? CADRE_BOOT_LAMPORTS;
    if (amt <= 0n || !this.sovereign) return "off";
    const b = this.cadreBoot;
    if (b?.state === "bridged") return "waiting_bridge";
    if (b?.state === "failed" && this.d.now() - b.at < 600_000) return "failed";
    try {
      if (!b || b.state === "failed") {
        const lam = await this.lamports();
        if (lam < amt + 20_000_000n) return "low_sol"; // keep 0.02 SOL for its x402 top-ups and Solana fees
        const before = await this.api.must("GET", `/v1/accounts/${this.address}/balances`).then((x: any) => BigInt(x.solana?.spl ?? 0));
        const r = await this.solanaAction("/v1/tx/pump-buy", { buyer: this.address, sol_in: amt.toString(), slippage_bps: 300 }, amt + 3_000_000n);
        let after = before;
        for (let i = 0; i < 10 && after <= before; i++) {
          after = await this.api.must("GET", `/v1/accounts/${this.address}/balances`).then((x: any) => BigInt(x.solana?.spl ?? 0));
          if (after <= before) await new Promise((res) => setTimeout(res, 1500));
        }
        if (after <= before) throw new Error(`the CADRE buy ${r.signature} landed but the SPL balance has not grown yet`);
        this.cadreBoot = { state: "bought", buy: r.signature, spl: (after - before).toString(), at: this.d.now() };
        this.note("cadre_buy", `bought ${after - before} CADRE base units with ${amt} lamports of its own SOL for its Cadre fees`, { sol: r.signature, ok: true });
        this.save();
      }
      const c = this.cadreBoot!;
      const r = await this.solanaAction("/v1/tx/bridge-deposit", { owner: this.address, recipient: this.address, amount_spl: c.spl }, 3_000_000n);
      this.cadreBoot = { ...c, state: "bridged", bridge: r.signature, at: this.d.now() };
      this.note("cadre_bridge", "bridged that CADRE to its own Cadre account (arrives after Solana finality and the relayer)", { sol: r.signature, ok: true });
      this.save();
      return "bridged";
    } catch (e) {
      this.cadreBoot = { ...(this.cadreBoot ?? {}), state: this.cadreBoot?.state === "bought" ? "bought" : "failed", at: this.d.now(), error: (e as Error).message.slice(0, 200) } as any;
      this.note("cadre_boot_error", `could not fund its Cadre fees yet: ${(e as Error).message.slice(0, 200)}`, { ok: false });
      this.save();
      return "error";
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
      const boot = await this.selfFundCadre();
      this.why = boot === "bridged" || boot === "waiting_bridge" ? "its own CADRE for Cadre fees is on the way (bridged from Solana; arrives after finality and the relayer)"
        : boot === "low_sol" ? "waiting for SOL in its wallet (it buys its own CADRE for Cadre fees from it)"
        : "the agent holds no CADRE on the Cadre chain for its tx fees yet";
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
    // the town on Solana: a creator link already verified on the Cadre chain still attributes the agent
    // until the creator links again on Solana (docs/TOWN-ON-SOLANA.md)
    const legacyLinked = isTownSolana(this.d.node) && !(prof?.operator === this.spec.operator && prof?.operator_linked)
      ? await (this.d.node as TownNode).chainCall("getProfile", { account: this.address }).then((p: any) => p?.operator === this.spec.operator && !!p?.operator_linked).catch(() => false)
      : false;
    // a sovereign (v2) agent's creator attribution is the creator-signed spec in its own profile blob
    // (verified at creation and by anyone from the chain); it does not wait for an OperatorLink
    const specAttributed = this.sovereign && prof?.operator === this.spec.operator;
    if (!specAttributed && !legacyLinked && !(prof?.operator === this.spec.operator && prof?.operator_linked)) {
      this.state = "waiting_link";
      this.why = "the creator's OperatorLink (attribution) to this agent is not on the chain yet";
      // an operator new to the Cadre chain holds no CADRE for the link's fee: the agent sends it once
      if (!this.linkFeeTx && isTownSolana(this.d.node)) this.linkFeeTx = "not-needed"; // the creator links on Solana with its own SOL
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
      this.liveAt = this.d.now();
      this.note("live", `running spec v${this.spec.version}; ${specAttributed && !prof?.operator_linked ? "creator attribution by the creator-signed spec in its profile blob" : "creator link verified on the chain"}`);
    }
    if (!this.paused) this.state = "running";
    return true;
  }

  /** "" when `what` may run now, else the protocol halt's reason (halt.ts; ALL stops everything). */
  haltReason(what: "all" | "trade" | "launch"): string {
    return this.o.halt?.blocks(what) ?? "";
  }

  /** Event-driven (owner 2026-10-22): an agent acts now, not at its next cadence slot, when work
   * waits for it: a hire to deliver (offer), a delivery to judge (hire), or a reply to answer.
   * DMs are already handled on every tick (Swarm.tick, dms.step). Checked at most every 10 s. */
  async pollTriggers(): Promise<string | null> {
    const now = this.d.now();
    const s = this.ag(this.me);
    if (!s.profileDone || now - this.triggersAt < 10_000 || (s.nextAt ?? 0) <= now) return null;
    this.triggersAt = now;
    const A = this.spec.actions;
    const get = (p: string) => this.api.must("GET", p).catch(() => null);
    let why: string | null = null;
    if (A.offer) {
      const h = await get(`/v1/market/hires?provider=${this.address}&status=open`);
      if (h?.hires?.some((x: any) => x.blocks_left > 2)) why = "a hire to deliver";
    }
    if (!why && A.hire) {
      const h = await get(`/v1/market/hires?buyer=${this.address}&evaluator=${this.address}&status=delivered`);
      if (h?.hires?.some((x: any) => !x.voted?.includes(this.address))) why = "a delivery to judge";
    }
    if (!why && A.reply) {
      const posts = await this.townPosts().catch(() => []);
      if (this.mentions(this.me, posts).length) why = "a reply to answer";
    }
    if (why) this.wakeAt = now;
    return why;
  }

  override dueAgents(now: number): Agent[] {
    const due = super.dueAgents(now);
    if (due.length) return due;
    if (this.wakeAt && this.wakeAt <= now && this.ag(this.me).profileDone) {
      this.wakeAt = 0;
      return [this.me];
    }
    return [];
  }

  override async tick(): Promise<string> {
    if (this.paused && !this.sovereign) { this.state = "paused"; return "paused"; }
    await this.o.halt?.refresh();
    const hb = this.haltReason("all");
    if (hb) {
      if (this.state !== "halted") this.note("halt", hb, { ok: false });
      this.state = "halted";
      this.why = hb;
      return "halted";
    }
    await this.pollTriggers().catch(() => null);
    const r = await super.tick();
    if (r === "error") { this.state = "error"; this.why = this.lastError ?? ""; }
    if (this.state === "running") {
      await this.startTrading().catch((e) => this.note("trade_error", `trading did not start: ${(e as Error).message}`, { ok: false }));
      if (this.o.practice) await this.o.practice.ensure(this).catch((e) => this.note("practice_error", `practice trading did not start: ${(e as Error).message.slice(0, 200)}`, { ok: false }));
      await this.evolve().catch((e) => this.note("evolve_error", (e as Error).message, { ok: false }));
      await this.launchTick().catch((e) => this.note("launch_error", (e as Error).message.slice(0, 200), { ok: false }));
      await this.toolbeltTick().catch((e) => this.note("toolbelt_error", (e as Error).message.slice(0, 200), { ok: false }));
    }
    return r;
  }

  /** A paid thought that may be larger than one standard top-up ticket (toolbelt reports): when the
   * open tickets have fewer free rows than the thought's upper bound, buy one ticket big enough for
   * it (still through the swarm's x402 top-up, inside the spec's daily SOL cap). */
  async thinkSized(a: Agent, messages: { role: string; content: string }[], maxTokens: number, purpose: string) {
    try {
      return await this.think(a, messages, maxTokens, purpose);
    } catch (e) {
      // the gateway names the rows one ticket must have free ("no prepaid balance with N free rows")
      const m = /\b402\b[\s\S]*with (\d+) free rows/.exec(String((e as Error).message));
      if (!m) throw e;
      const need = Number(m[1]);
      const keep = this.cfg.topupRows;
      this.cfg.topupRows = Math.max(keep, need + this.cfg.minFreeRows);
      let ok = false;
      try { ok = await this.topup(); } finally { this.cfg.topupRows = keep; }
      this.note("inference_topup", ok ? `bought a ${Math.max(keep, need + this.cfg.minFreeRows)}-row prepaid ticket for a ${need}-row thought` : `could not top up for a ${need}-row thought (the spec's daily SOL cap, budget.sol_lamports_per_day, or the gateway refused)`, { ok });
      if (!ok) return null;
      return this.think(a, messages, maxTokens, purpose);
    }
  }

  /** v2 toolbelt standing work: the spec's watch list, and a receipt batch older than a day. */
  async toolbeltTick(): Promise<string> {
    const tb = this.toolbelt;
    if (!tb) return "off";
    if (tb.spec.watch) {
      const r = await toolWatch(tb, (m, n, p) => this.think(this.me, m, n, p).then((t) => (t ? { text: t.text, requestId: t.requestId } : null)), this.watchSt, this.d.now(), this.me.persona.persona);
      for (const c of r.changed) {
        if (!c.summary || !this.spec.actions.post) continue;
        const text = `changed: ${c.url.replace(/^https:\/\//, "")}: ${c.summary}`.slice(0, 900);
        if (!checkOutput(text, { maxBytes: 1024, allowHosts: [new URL(c.url).hostname, ...this.cfg.allowHosts] }).ok) continue;
        const h = await this.send(this.me.wallet, { kind: "Post", postKind: 0, body: text, thoughtRef: c.thought ?? undefined });
        this.note("watch", text, { tx: h, request_id: c.thought ?? undefined, ok: true });
      }
      if (r.due) this.save();
    }
    const old = tb.pending[0];
    if (old && this.d.now() - old.started_ms > 86_400_000) await tb.maybeFlush(true);
    return "ok";
  }

  /** The v2 launch skill (launch.ts), when the spec opts in. */
  async launchTick(): Promise<string> {
    const l = this.spec.launch;
    if (!this.sovereign || !l || !this.spec.actions.launch || !this.spec.skills.includes("launch")) return "off";
    const me = this;
    const host: LaunchHost = {
      address: this.address, handle: this.spec.handle, operator: this.spec.operator, specSha256: this.doc.sha256, persona: this.me.persona.persona,
      apiUrl: (this.api as any).base as string, wallet: this.me.wallet, launch: l, launches: this.launches,
      get consideredAt() { return me.launchConsideredAt; },
      set consideredAt(v: number) { me.launchConsideredAt = v; },
      now: () => this.d.now(),
      haltLaunch: () => this.haltReason("launch"),
      canSpendSol: (x) => this.budget.canTopup(x, this.d.now()),
      bookSol: (x) => this.budget.bookTopup(x),
      lamports: () => this.lamports(),
      record: () => `counters ${JSON.stringify(this.ag(this.me).counters ?? {})}; tokens launched before: ${this.launches.map((x) => x.symbol).join(", ") || "none"}`,
      think: async (m, n) => { const t = await this.think(this.me, m, n, "launch"); return t ? { text: t.text, requestId: t.requestId } : null; },
      storeMeta: async (m) => { const r = await this.api.must("POST", "/v1/launches/metadata", m); return { id: r.id as string }; },
      build: (body, signers) => this.api.solanaMulti(signers, "/v1/tx/pump-create", body),
      confirm: (sig) => this.api.confirmSolana(sig),
      receipt: (text, sigs) => this.receipt(text, sigs),
      note: (k, t, x) => this.note(k, t, x as Partial<LogLine>),
      save: () => this.save(),
      tradeOwn: async (mint) => { if (this.trading) await this.trading.addOwnToken(mint); },
    };
    const r = await maybeLaunch(host);
    if (r === "launched") this.count(this.me, "launches");
    return r;
  }

  /** The v2 trade skill: the full trading engine from the agent's own wallet (once, when running). */
  async startTrading() {
    const t = this.spec.trade;
    if (this.trading || !this.sovereign || !t || !this.spec.actions.trade || !this.spec.skills.includes("trade")) return;
    if (!realSwapsOn(this.spec)) return; // practice instead (practice.ts); real swaps only with trade.real_swaps
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
      halt: () => (this.state === "running" ? this.haltReason("trade") : this.why || `agent is ${this.state}`),
      log: (e, x) => this.log(e, x),
    });
    await this.trading.start();
    if (this.spec.launch?.trade_own) for (const x of this.launches) await this.trading.addOwnToken(x.mint).catch((e) => this.note("trade_error", `own token ${x.mint} not added: ${(e as Error).message}`, { ok: false }));
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
    this.me.persona = personaOf(this.spec, self, this.swarm);
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
    // services market (opt-in, docs/MARKET.md 7): paid work and own verdicts come first
    const mk = (await this.market.work(a)) ?? (await this.market.judgeOwn(a)) ?? (await this.market.ensureOffers());
    if (mk) return mk;
    // a swarm lead acts on its kickoff plan first: hire the member it chose for the goal
    if (this.swarmPlan && A.hire) { const h = await this.market.hire(a); if (h && h !== "idle") return h; }
    const opts: [string, number][] = [];
    if (A.hire) opts.push(["hire", this.swarm?.isLead && this.swarm.members.length > 1 ? 0.2 : 0.05]); // a swarm lead hires its members more often (docs/SWARMS.md 4)
    if ((A.react || A.tip) && others.length) opts.push(["react", 0.2]);
    if (A.follow && others.length) opts.push(["follow", 0.1]);
    if (A.reply && others.length) opts.push(["reply", 0.25]);
    // trading is practice for user agents (practice.ts); the v1 single PumpSwap buy (trade()) stays in
    // the code but no user agent reaches it
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
      case "hire": return (await this.market.hire(a)) ?? "idle";
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
      spec: { version: this.spec.version, sha256: this.doc.sha256, profile_uri: this.blobUri, hosting: this.spec.hosting, template: this.spec.template ?? null },
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
      trading: this.trading ? this.trading.status() : realSwapsOn(this.spec) ? { state: "not started", why: this.state === "running" ? "starting" : this.why || this.state } : null,
      practice: this.o.practice?.status(this.address) ?? (practicePlan(this.spec) ? { mode: "practice", state: "not started", why: this.o.practice ? (this.state === "running" ? "starting" : this.why || this.state) : this.o.practiceOff ?? "practice is off on this runtime" } : null),
      cadre_self_funding: this.cadreBoot,
      timings: { live_ms: this.liveAt || null, first_thought_ms: this.firsts.thought ?? null, first_tx_ms: this.firsts.tx ?? null, first_task_ms: this.firsts.task ?? null },
      toolbelt: this.toolbelt ? this.toolbelt.status() : null,
      launches: this.spec.launch ? { bounds: this.spec.launch, launched: this.launches, considered_at: this.launchConsideredAt || null, label: "agent-launched (pump.fun create_v2 by this agent's key)" } : null,
      halt: this.o.halt ? this.o.halt.status() : null,
      swarm: this.swarm ? { key: this.swarm.key, id: this.swarm.id, name: this.swarm.name, role: this.swarm.role, is_lead: this.swarm.isLead } : null,
      log: this.log2.slice(-60).reverse(),
    };
  }
}
