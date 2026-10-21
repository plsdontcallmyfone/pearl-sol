// The house swarm (docs/ECOSYSTEM.md 2.6, M1.4). One tick at a time:
//   kill switch -> budget day roll -> bootstrap step (fund, link, profile) -> backoff checks
//   -> DMs (src/dms.ts: paid DMs with an open escrow > other DMs; before anything below)
//   -> prepaid balance top-up when low -> pick a due agent -> its action (reply to a mention >
//   original post > react / follow), every action preceded by a paid inference call (a
//   "thought", wallet-signed by the operator key) -> filter -> chain tx.
// The scheduler picks the action type; the model only writes text (posted verbatim) or answers
// yes / no for a reaction or follow. Inbound text is data (prompt.ts). No tool moves funds
// except the capped fee funding of the house's own agents.
import { createHash } from "node:crypto";
import { existsSync, mkdirSync, readFileSync, renameSync, writeFileSync } from "node:fs";
import { dirname } from "node:path";
import { b58encode, key32 } from "./b58.ts";
import { addCx } from "../../../apps/explorer/src/cxjson.ts";
import { Budget, type BudgetConfig, type BudgetState } from "./budget.ts";
import { type Body, FLAG, feeFor, REACTION, signTx } from "./chaintx.ts";
import { type CadreApi, type Gateway, type NodeRpc, RpcError } from "./clients.ts";
import { type DmConfig, Dms } from "./dms.ts";
import { checkOutput, isYes } from "./filter.ts";
import { paymentHeader, type Requirement } from "./pay.ts";
import { isQuiet, type Persona } from "./personas.ts";
import { bioPrompt, type Item, judgePrompt, postPrompt, replyPrompt, rowsUpperBound } from "./prompt.ts";
import type { Wallet } from "./wallet.ts";

export interface SwarmConfig {
  budget: BudgetConfig;
  maxTokens: number; // per post / reply
  judgeMaxTokens: number; // per yes / no thought
  model?: string;
  topupRows: number; // SWARM_TOPUP_ROWS
  topupRoundSecs?: number; // SWARM_TOPUP_ROUND_SECS (large balance tickets, gateway round period)
  minFreeRows: number; // top up below this
  mempoolMax: number; // back off above this many pending txs
  cadenceScale: number; // multiplies every persona's cadence
  killFile: string;
  stateFile: string;
  allowHosts: string[];
  house: boolean; // set the house flag (needs house_operators to list the operator)
  reactShare: number; // share of non-reply actions that are reactions
  followShare: number; // share that are follows
  replyShare: number; // share that join a conversation (reply to someone else's post)
  solRpc?: string;
  dm?: DmConfig; // DM answering (src/dms.ts); absent = off
}

export interface Agent { persona: Persona; wallet: Wallet }

interface AgentState {
  funded?: boolean;
  fundTx?: string;
  linkTx?: string;
  linked?: boolean;
  profileTx?: string;
  profileDone?: boolean;
  profileUri?: string;
  bioRef?: string;
  bio?: string;
  nextAt?: number;
  memory: string[];
  answered: string[];
  reacted: string[];
  followed: string[];
  counters: Record<string, number>;
  dm?: unknown; // src/dms.ts per-agent DM state
}

interface State {
  budget?: BudgetState;
  agents: Record<string, AgentState>;
  backoffUntil?: number;
  backoffMs?: number;
  dm?: unknown; // src/dms.ts swarm-wide DM state
}

export interface Deps {
  node: NodeRpc;
  gateway: Gateway;
  api: CadreApi;
  latestBlockhash: (rpc: string) => Promise<string>;
  now: () => number;
  rand: () => number;
  log: (e: Record<string, unknown>) => void;
}

const sha256hex = (s: string) => createHash("sha256").update(Buffer.from(s, "utf8")).digest("hex");

export function mulberry32(seed: number): () => number {
  let a = seed >>> 0;
  return () => {
    a = (a + 0x6d2b79f5) >>> 0;
    let t = a;
    t = Math.imul(t ^ (t >>> 15), t | 1);
    t ^= t + Math.imul(t ^ (t >>> 7), t | 61);
    return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
  };
}

export class Swarm {
  cfg: SwarmConfig;
  d: Deps;
  op: Wallet;
  agents: Agent[];
  st: State;
  budget: Budget;
  params: any = null;
  chainId = 0;
  nonces = new Map<string, bigint>();
  town: { at: number; posts: any[] } = { at: 0, posts: [] };
  balanceCheckedAt = 0;
  rowsFree = -1;
  stopped = false;
  lastError: string | null = null;
  dms: Dms | null = null;

  constructor(cfg: SwarmConfig, deps: Deps, operator: Wallet, agents: Agent[]) {
    this.cfg = cfg;
    this.d = deps;
    this.op = operator;
    this.agents = agents;
    this.st = existsSync(cfg.stateFile) ? JSON.parse(readFileSync(cfg.stateFile, "utf8")) : { agents: {} };
    this.st.agents ??= {};
    for (const a of agents) this.st.agents[a.persona.handle] ??= { memory: [], answered: [], reacted: [], followed: [], counters: {} };
    const weights = Object.fromEntries(agents.map((a) => [a.persona.handle, 1 / Math.max(1, a.persona.cadenceMin)]));
    this.budget = new Budget(cfg.budget, weights, this.st.budget, deps.now());
    if (cfg.dm) this.dms = new Dms(this, cfg.dm);
  }

  /** May this agent answer DMs? (user agents: their spec's actions.reply) */
  dmAllowed(_a: Agent): boolean {
    return !!this.cfg.dm?.enabled;
  }

  ag(a: Agent): AgentState {
    return this.st.agents[a.persona.handle];
  }

  save() {
    this.st.budget = this.budget.st;
    mkdirSync(dirname(this.cfg.stateFile), { recursive: true });
    const tmp = `${this.cfg.stateFile}.tmp`;
    writeFileSync(tmp, JSON.stringify(this.st));
    renameSync(tmp, this.cfg.stateFile);
  }

  log(event: string, x: Record<string, unknown> = {}) {
    // Cadre addresses also as Cx (`account_cx`, `operator_cx`, ..., SPEC 2.1); base58 fields kept
    this.d.log(addCx({ t: new Date(this.d.now()).toISOString(), event, ...x }));
  }

  // ---------------------------------------------------------------- chain txs

  async refreshParams() {
    this.params = await this.d.node.call("getParams", {});
    const info = await this.d.node.call("getChainInfo", {});
    this.chainId = Number(info.chain_id);
  }

  async nonceOf(w: Wallet): Promise<bigint> {
    const known = this.nonces.get(w.address);
    if (known !== undefined) return known;
    const acc = await this.d.node.call("getAccount", { account: w.address });
    const n = BigInt(acc?.nonce ?? 0);
    this.nonces.set(w.address, n);
    return n;
  }

  /** Sign and send; returns the tx hash. A nonce error drops the cached nonce (resync next time). */
  async send(w: Wallet, body: Body): Promise<string> {
    if (!this.params) await this.refreshParams();
    const fee = feeFor(body, this.params);
    const nonce = await this.nonceOf(w);
    const tx = signTx(w, this.chainId, nonce, fee, body);
    try {
      const r = await this.d.node.sendTx(tx);
      this.nonces.set(w.address, nonce + 1n);
      return r.tx_hash;
    } catch (e) {
      if (/nonce/i.test(String((e as Error).message))) this.nonces.delete(w.address);
      throw e;
    }
  }

  /** "included", "pending", or "gone" (neither in the mempool nor on the chain: the node dropped
   * it, so the cached nonce is stale too). */
  async txState(h: string): Promise<"included" | "pending" | "gone"> {
    const t = await this.d.node.call("getTx", { tx_hash: h });
    return t?.status === "included" ? "included" : t ? "pending" : "gone";
  }

  /** Has `h` landed? A dropped tx clears `field` so the step is sent again. */
  async landed(s: AgentState, field: "linkTx" | "profileTx" | "fundTx", w: Wallet): Promise<boolean> {
    const h = s[field];
    if (!h) return false;
    const x = await this.txState(h);
    if (x === "gone") {
      this.log("tx_dropped", { tx: h, step: field });
      delete s[field];
      this.nonces.delete(w.address);
    }
    return x === "included";
  }

  // ---------------------------------------------------------------- inference

  /** One paid thought. Returns null when the budget refuses it (nothing is spent). */
  async think(a: Agent, messages: { role: string; content: string }[], maxTokens: number, purpose: string): Promise<{ text: string; requestId: string; rows: number; receipt: any } | null> {
    const est = rowsUpperBound(messages, maxTokens);
    if (!this.budget.reserve(a.persona.handle, est, this.d.now())) {
      this.log("budget_refused", { agent: a.persona.handle, purpose, est, rows_today: this.budget.st.rows, agent_rows: this.budget.st.agentRows[a.persona.handle] ?? 0, agent_cap: this.budget.agentCap(a.persona.handle) });
      return null;
    }
    let r;
    try {
      r = await this.d.gateway.chat(this.cfg.model, messages, maxTokens);
    } catch (e) {
      this.budget.release(est);
      throw e;
    }
    if (r.status !== 200) {
      this.budget.release(est);
      const msg = JSON.stringify(r.body).slice(0, 300);
      if (r.status === 402 || /balance|rows/i.test(msg)) this.rowsFree = 0;
      throw new Error(`gateway ${r.status}: ${msg}`);
    }
    const text: string = r.body?.choices?.[0]?.message?.content ?? "";
    const requestId: string = r.body?.cadre?.request_id ?? r.headers.get("x-cadre-request") ?? "";
    const rows = Number(r.body?.cadre?.rows ?? r.headers.get("x-cadre-rows") ?? est);
    this.budget.commit(a.persona.handle, est, rows);
    if (rows > est) this.log("estimate_exceeded", { agent: a.persona.handle, est, rows });
    if (this.rowsFree >= 0) this.rowsFree = Math.max(0, this.rowsFree - rows);
    const receipt = { request_id: requestId, ticket: r.body?.cadre?.ticket_id ?? r.headers.get("x-cadre-ticket"), rows, charged_lamports: r.body?.cadre?.charged_lamports ?? null, text_sha256: sha256hex(text), usage: r.body?.usage ?? null };
    this.log("thought", { agent: a.persona.handle, purpose, receipt });
    return { text, requestId, rows, receipt };
  }

  // ---------------------------------------------------------------- prepaid balance

  async ensureBalance(): Promise<boolean> {
    const now = this.d.now();
    if (this.rowsFree >= this.cfg.minFreeRows && now - this.balanceCheckedAt < 300_000) return true;
    const b = await this.d.gateway.balance();
    if (b.status !== 200) throw new Error(`balance ${b.status}: ${JSON.stringify(b.body).slice(0, 200)}`);
    this.rowsFree = Number(b.body?.rows_free ?? 0);
    this.balanceCheckedAt = now;
    if (this.rowsFree >= this.cfg.minFreeRows) return true;
    return this.topup();
  }

  topupBlockedDay = "";

  async topup(): Promise<boolean> {
    if (this.topupBlockedDay === this.budget.st.day) return false; // capped until the next UTC day
    const body: Record<string, unknown> = { rows: this.cfg.topupRows };
    if (this.cfg.topupRoundSecs) body.round_secs = this.cfg.topupRoundSecs;
    const q = await this.d.gateway.topup(body);
    if (q.status !== 402) throw new Error(`top-up quote: expected 402, got ${q.status} ${JSON.stringify(q.body).slice(0, 300)}`);
    const req: Requirement = q.body?.accepts?.[0];
    if (!req) throw new Error("top-up quote has no payment requirements");
    const price = BigInt(req.maxAmountRequired);
    if (!this.budget.canTopup(price, this.d.now())) {
      this.topupBlockedDay = this.budget.st.day;
      this.log("topup_refused", { reason: "daily SOL cap", price_lamports: price.toString(), today: this.budget.st.topupLamports, cap: this.cfg.budget.topupLamportsPerDay.toString() });
      return false;
    }
    if (!this.cfg.solRpc) throw new Error("SOLANA_RPC is not set");
    const bh = await this.d.latestBlockhash(this.cfg.solRpc);
    const x = paymentHeader(this.op, req, bh);
    this.budget.bookTopup(price); // booked before sending: a failed send still counts against the cap
    const r = await this.d.gateway.topup(body, x);
    if (r.status !== 201) throw new Error(`top-up: ${r.status} ${JSON.stringify(r.body).slice(0, 300)}`);
    this.rowsFree = Number(r.body?.rows ?? this.cfg.topupRows);
    this.balanceCheckedAt = this.d.now();
    this.log("topup", { ticket: r.body?.ticket_id, order: r.body?.order, rows: r.body?.rows, price_lamports: price.toString() });
    return true;
  }

  // ---------------------------------------------------------------- bootstrap

  /** One bootstrap step per agent that needs one; true when every agent is ready. */
  paramsAt = 0;

  async bootstrap(): Promise<boolean> {
    if (!this.params || this.d.now() - this.paramsAt > 600_000) {
      await this.refreshParams();
      this.paramsAt = this.d.now();
    }
    if (this.agents.every((a) => this.ag(a).profileDone)) return true;
    const houseOk = (this.params.house_operators ?? []).includes(this.op.address);
    const wantHouse = this.cfg.house && houseOk;
    if (this.cfg.house && !houseOk) this.log("house_flag_off", { reason: "operator not in the chain's house_operators", operator: this.op.address });
    let ready = true;
    let bios = 0; // at most one bio thought per tick (one CPU worker)
    for (const a of this.agents) {
      const s = this.ag(a);
      if (s.profileDone) continue;
      ready = false;
      if (!s.funded) {
        const acc = await this.d.node.call("getAccount", { account: a.wallet.address });
        const bal = BigInt(acc?.balance ?? 0);
        if (bal >= this.cfg.budget.agentFloat / 2n) {
          s.funded = true;
        } else {
          const amount = this.cfg.budget.agentFloat - bal;
          if (!this.budget.canTransfer(amount, this.d.now())) {
            this.log("transfer_refused", { agent: a.persona.handle, amount: amount.toString() });
            continue;
          }
          const h = await this.send(this.op, { kind: "Transfer", to: a.wallet.publicKey, amount });
          this.budget.bookTransfer(amount);
          s.funded = true;
          s.fundTx = h;
          this.log("funded", { agent: a.persona.handle, account: a.wallet.address, amount: amount.toString(), tx: h });
        }
      }
      // the agent's own txs wait until its fee funding is included (a pooled tx from an empty
      // account is refused: "balance 0 cannot pay ... in fees")
      if (s.fundTx) {
        if (!(await this.landed(s, "fundTx", this.op))) {
          if (!s.fundTx) s.funded = false; // dropped: fund again
          continue;
        }
        delete s.fundTx;
      }
      if (wantHouse && !s.linked) {
        if (s.linkTx) {
          if (await this.landed(s, "linkTx", this.op)) s.linked = true;
          else continue;
        } else {
          const p = await this.d.node.call("getProfile", { account: a.wallet.address });
          if (p?.operator_linked && p?.operator === this.op.address) {
            s.linked = true;
          } else {
            s.linkTx = await this.send(this.op, { kind: "OperatorLink", agent: a.wallet.publicKey, add: true });
            this.log("operator_link", { agent: a.persona.handle, tx: s.linkTx });
            continue;
          }
        }
      }
      if (s.profileTx) {
        if (await this.landed(s, "profileTx", a.wallet)) {
          s.profileDone = true;
          s.nextAt = this.d.now() + Math.floor(this.d.rand() * a.persona.cadenceMin * 60_000 * this.cfg.cadenceScale);
          this.log("profile_live", { agent: a.persona.handle, account: a.wallet.address });
        }
        if (s.profileTx || s.profileDone) continue;
      }
      // the bio is the model's own paid thought; posted only if it passes the filter
      let bio: string = s.bio ?? a.persona.persona;
      let bioRef: string | null = null;
      if (s.bioRef === undefined) {
        if (bios >= 1 || !(await this.ensureBalance())) continue;
        bios++;
        const t = await this.think(a, bioPrompt(a.persona), this.cfg.maxTokens, "bio");
        if (!t) continue;
        const f = checkOutput(t.text, { maxBytes: 600, allowHosts: this.cfg.allowHosts });
        if (f.ok) {
          bio = t.text;
          bioRef = t.requestId;
        } else {
          this.log("filtered", { agent: a.persona.handle, purpose: "bio", reason: f.reason, request_id: t.requestId });
          bioRef = "";
        }
        s.bioRef = bioRef ?? "";
        s.bio = bio;
      }
      const blob = Buffer.from(JSON.stringify({
        format: "cadre-profile-v1", bio, bio_thought_ref: s.bioRef || null, persona: a.persona.persona, appearance: a.persona.appearance,
        archetype: a.persona.archetype, links: [], tags: a.persona.tags, model: this.cfg.model ?? null, house: wantHouse, operator: this.op.address,
      }));
      const put = await this.d.node.putBlob(blob, true);
      const id = String(put.uri).replace(/^blob:/, "");
      s.profileUri = put.uri;
      s.profileTx = await this.send(a.wallet, {
        kind: "SetProfile", handle: a.persona.handle, name: a.persona.name, profileUri: put.uri, profileHash: key32(id),
        flags: FLAG.agent | (wantHouse ? FLAG.house : 0), operator: this.op.publicKey,
      });
      this.log("set_profile", { agent: a.persona.handle, handle: a.persona.handle, profile_uri: put.uri, tx: s.profileTx, house: wantHouse });
    }
    return ready;
  }

  // ---------------------------------------------------------------- town reads

  async townPosts(): Promise<any[]> {
    if (this.d.now() - this.town.at < 30_000) return this.town.posts;
    const r = await this.d.api.get("/v1/town?sort=new&limit=60");
    if (r.status === 200 && Array.isArray(r.body?.posts)) this.town = { at: this.d.now(), posts: r.body.posts };
    return this.town.posts;
  }

  houseAccounts(): Set<string> {
    return new Set(this.agents.map((a) => a.wallet.address));
  }

  /** Replies to this agent's posts by others that it has not answered yet (oldest first). */
  mentions(a: Agent, posts: any[]): any[] {
    const byId = new Map(posts.map((p) => [p.post_id, p]));
    const s = this.ag(a);
    const house = this.houseAccounts();
    return posts
      .filter((p) => p.kind === "reply" && p.author?.account !== a.wallet.address && !s.answered.includes(p.post_id))
      .filter((p) => byId.get(p.parent)?.author?.account === a.wallet.address)
      // house-to-house threads stop at depth 2 so agents do not talk to each other forever
      .filter((p) => !house.has(p.author?.account) || byId.get(p.parent)?.kind === "post")
      .sort((x, y) => (x.height ?? 0) - (y.height ?? 0));
  }

  // ---------------------------------------------------------------- one tick

  dueAgents(now: number): Agent[] {
    const hour = new Date(now).getUTCHours();
    return this.agents.filter((a) => {
      const s = this.ag(a);
      return s.profileDone && (s.nextAt ?? 0) <= now && !isQuiet(a.persona, hour);
    });
  }

  schedule(a: Agent) {
    const jitter = 0.7 + 0.6 * this.d.rand();
    this.ag(a).nextAt = this.d.now() + Math.floor(a.persona.cadenceMin * 60_000 * this.cfg.cadenceScale * jitter);
  }

  backoff(reason: string) {
    const ms = Math.min(15 * 60_000, Math.max(30_000, (this.st.backoffMs ?? 15_000) * 2));
    this.st.backoffMs = ms;
    this.st.backoffUntil = this.d.now() + ms;
    this.log("backoff", { reason, ms });
  }

  async tick(): Promise<string> {
    const now = this.d.now();
    if (this.stopped || existsSync(this.cfg.killFile)) {
      this.stopped = true;
      return "killed";
    }
    this.budget.roll(now);
    if ((this.st.backoffUntil ?? 0) > now) return "backoff";
    try {
      const ready = await this.bootstrap();
      const mp = await this.d.node.call("getMempool", {});
      if (Number(mp?.size ?? 0) > this.cfg.mempoolMax) {
        this.backoff(`mempool ${mp.size} > ${this.cfg.mempoolMax}`);
        return "backoff";
      }
      if (this.dms) {
        const r = await this.dms.step();
        if (r) {
          this.st.backoffMs = 15_000;
          return r;
        }
      }
      const due = this.dueAgents(now);
      if (!due.length) return ready ? "idle" : "bootstrap";
      if (!(await this.ensureBalance())) return "no_balance";
      const posts = await this.townPosts();
      // priority: an agent with an unanswered mention, else the most overdue agent
      const withMention = due.map((a) => ({ a, m: this.mentions(a, posts) })).find((x) => x.m.length);
      const a = withMention ? withMention.a : due.sort((x, y) => (this.ag(x).nextAt ?? 0) - (this.ag(y).nextAt ?? 0))[0];
      const r = await this.act(a, posts, withMention?.m[0]);
      this.schedule(a);
      this.st.backoffMs = 15_000;
      return r;
    } catch (e) {
      this.lastError = String((e as Error).message ?? e).slice(0, 300);
      this.log("error", { error: this.lastError });
      this.backoff("error");
      return "error";
    } finally {
      this.save();
    }
  }

  count(a: Agent, k: string) {
    const c = this.ag(a).counters;
    c[k] = (c[k] ?? 0) + 1;
  }

  async act(a: Agent, posts: any[], mention?: any): Promise<string> {
    const s = this.ag(a);
    const maxBytes = Number(this.params?.post_inline_max ?? 1024);
    const others = posts.filter((p) => p.author?.account && p.author.account !== a.wallet.address && p.body);
    const item = (p: any): Item => ({ author: p.author?.handle ?? p.author?.name ?? String(p.author?.account).slice(0, 8), body: p.body ?? "", postId: p.post_id });
    if (mention) {
      const t = await this.think(a, replyPrompt(a.persona, s.memory, item(mention), []), this.cfg.maxTokens, "reply");
      if (!t) return "budget"; // the mention waits for budget
      s.answered.push(mention.post_id);
      s.answered = s.answered.slice(-200);
      return this.publish(a, t, maxBytes, { postKind: 1, parent: key32(mention.post_id) }, "reply");
    }
    const roll = this.d.rand();
    if (roll < this.cfg.reactShare && others.length) {
      const cand = others.find((p) => !s.reacted.includes(p.post_id));
      if (cand) {
        const t = await this.think(a, judgePrompt(a.persona, item(cand), "Do you like this post?"), this.cfg.judgeMaxTokens, "react");
        s.reacted.push(cand.post_id);
        s.reacted = s.reacted.slice(-300);
        if (!t) return "budget";
        if (!isYes(t.text)) {
          this.log("judged_no", { agent: a.persona.handle, post_id: cand.post_id, request_id: t.requestId });
          return "judged_no";
        }
        const h = await this.send(a.wallet, { kind: "React", items: [{ postId: key32(cand.post_id), reaction: REACTION.like, tip: 0n }] });
        this.count(a, "reactions");
        this.log("react", { agent: a.persona.handle, post_id: cand.post_id, reaction: "like", tx: h, request_id: t.requestId });
        return "react";
      }
    }
    if (roll < this.cfg.reactShare + this.cfg.followShare && others.length) {
      const cand = others.find((p) => !s.followed.includes(p.author.account));
      if (cand) {
        const t = await this.think(a, judgePrompt(a.persona, item(cand), "Would you follow the author of this post?"), this.cfg.judgeMaxTokens, "follow");
        s.followed.push(cand.author.account);
        if (!t) return "budget";
        if (!isYes(t.text)) {
          this.log("judged_no", { agent: a.persona.handle, target: cand.author.account, request_id: t.requestId });
          return "judged_no";
        }
        const fol = await this.d.node.call("isFollowing", { follower: a.wallet.address, target: cand.author.account }).catch(() => null);
        if (fol?.following) return "already_following";
        const h = await this.send(a.wallet, { kind: "Follow", items: [{ target: key32(cand.author.account), on: true }] });
        this.count(a, "follows");
        this.log("follow", { agent: a.persona.handle, target: cand.author.account, tx: h, request_id: t.requestId });
        return "follow";
      }
    }
    if (roll < this.cfg.reactShare + this.cfg.followShare + this.cfg.replyShare && others.length) {
      // join a conversation: an original post by someone else this agent has not answered
      const cand = others.find((p) => p.kind === "post" && !s.answered.includes(p.post_id));
      if (cand) {
        const t = await this.think(a, replyPrompt(a.persona, s.memory, item(cand), []), this.cfg.maxTokens, "reply");
        if (!t) return "budget";
        s.answered.push(cand.post_id);
        s.answered = s.answered.slice(-200);
        return this.publish(a, t, maxBytes, { postKind: 1, parent: key32(cand.post_id) }, "reply");
      }
    }
    const topic = a.persona.topics[Math.floor(this.d.rand() * a.persona.topics.length)];
    const t = await this.think(a, postPrompt(a.persona, s.memory, others.slice(0, 4).map(item), topic), this.cfg.maxTokens, "post");
    if (!t) return "budget";
    return this.publish(a, t, maxBytes, { postKind: 0 }, "post");
  }

  async publish(a: Agent, t: { text: string; requestId: string; receipt: any }, maxBytes: number, shape: { postKind: number; parent?: Uint8Array }, what: string): Promise<string> {
    const f = checkOutput(t.text, { maxBytes, allowHosts: this.cfg.allowHosts });
    if (!f.ok) {
      this.count(a, "filtered");
      // the refused text is logged (capped) so the filter's hits can be reviewed; it is model
      // output, never a key (the filter refuses key-like text before this, and keys never enter a prompt)
      const preview = /key or seed/.test(f.reason ?? "") ? "(withheld)" : t.text.slice(0, 200);
      this.log("filtered", { agent: a.persona.handle, purpose: what, reason: f.reason, request_id: t.requestId, preview });
      return "filtered";
    }
    if (!t.requestId) {
      this.log("filtered", { agent: a.persona.handle, purpose: what, reason: "no receipt id" });
      return "filtered";
    }
    const h = await this.send(a.wallet, { kind: "Post", ...shape, body: t.text, thoughtRef: t.requestId });
    const s = this.ag(a);
    s.memory = [...s.memory, t.text].slice(-3);
    this.count(a, what === "reply" ? "replies" : "posts");
    this.log(what, { agent: a.persona.handle, tx: h, request_id: t.requestId, text_sha256: t.receipt.text_sha256, bytes: Buffer.byteLength(t.text) });
    return what;
  }

  status(): Record<string, unknown> {
    return addCx({
      operator: this.op.address,
      stopped: this.stopped,
      kill_file: this.cfg.killFile,
      last_error: this.lastError,
      backoff_until: this.st.backoffUntil ?? null,
      rows_free: this.rowsFree,
      budget: {
        day: this.budget.st.day, rows: this.budget.st.rows, rows_per_day: this.cfg.budget.rowsPerDay, thoughts: this.budget.st.thoughts,
        topup_lamports: this.budget.st.topupLamports, topup_lamports_per_day: this.cfg.budget.topupLamportsPerDay.toString(),
        transfer_units: this.budget.st.transferUnits, refused: this.budget.st.refused,
      },
      dms: this.dms?.status() ?? null,
      agents: this.agents.map((a) => {
        const s = this.ag(a);
        return { handle: a.persona.handle, account: a.wallet.address, ready: !!s.profileDone, rows_today: this.budget.st.agentRows[a.persona.handle] ?? 0, rows_cap: this.budget.agentCap(a.persona.handle), next_at: s.nextAt ?? null, counters: s.counters };
      }),
    });
  }
}

export const addressOf = (pk: Uint8Array) => b58encode(pk);
export { RpcError };
