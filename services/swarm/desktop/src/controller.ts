// The house desktops controller. It runs the house swarm (services/swarm, unchanged) for the
// agents that have desktops, with one difference: an agent's chain action (post, reply, like, tip,
// follow) is not signed by this process; the agent's own social desktop does it, in the browser,
// through the Cadre site and the Cadre Wallet, operated by a vision model. Every word an agent
// says still comes from a paid Cadre inference request (the swarm's `think`), and the desktop
// types it verbatim with the request id as thought_ref. Between chain actions the desktops browse
// (read-only): the town, profiles and leaderboards on the social desktop; the wallet, the
// explorer, the trading terminal and the AI page on the work desktop.
//
// One identity spans both desktops: one key, one persona. Only the social desktop signs, one
// action at a time (the swarm's tick is sequential), under the agent's signing lock, so the two
// desktops never race for the wallet nonce; work tasks are read-only until Phases 2-4.
import { existsSync, mkdirSync, readFileSync } from "node:fs";
import { resolve } from "node:path";
import { b58encode } from "../../src/b58.ts";
import type { Body } from "../../src/chaintx.ts";
import { REACTION } from "../../src/chaintx.ts";
import { PERSONAS, type Persona } from "../../src/personas.ts";
import { type Agent, type Deps, Swarm, type SwarmConfig } from "../../src/swarm.ts";
import { loadWallet, type Wallet } from "../../src/wallet.ts";
import type { DeskConfig } from "./config.ts";
import { usdPerSecond } from "./config.ts";
import { Desk, type E2BApi, type Role } from "./desk.ts";
import { Ledger } from "./spend.ts";
import { browseTask, chainTask, type Task, workTask } from "./tasks.ts";
import { type Chat, runTask, systemPrompt } from "./vision.ts";

export class DeskFailed extends Error {}

/** Desktop agents are taken from the END of the house persona list (persona 30 first), so the
 * headless cadre-swarm keeps the first 30 - N personas with SWARM_AGENTS=30-N and no agent is
 * ever driven by both. */
export const DEFAULT_HANDLES = () => PERSONAS.slice(10).reverse().map((p) => p.handle);

export interface LogEntry { t: string; desk: Role | "agent"; kind: string; text: string; ref?: string; tx?: string; ok?: boolean }
export interface ThoughtEntry { t: string; purpose: string; request_id: string; rows: number; text_sha256: string; charged_lamports: string | null }

export interface DeskAgent {
  persona: Persona;
  wallet: Wallet;
  secretB58: string;
  password: string;
  desks: Record<Role, Desk>;
  log: LogEntry[];
  thoughts: ThoughtEntry[];
  current: Record<Role, string>;
  lastTaskAt: Record<Role, number>;
  busy: Record<Role, boolean>;
  signing: boolean;
  tipsToday: { day: string; n: number };
}

const now = () => Date.now();
const iso = () => new Date().toISOString();
const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms));

/** Reads a Solana-style 64-byte JSON key file as base58 (only ever written into the agent's own desktop). */
export function secretB58(file: string): string {
  const a = JSON.parse(readFileSync(file, "utf8"));
  if (!Array.isArray(a) || a.length !== 64) throw new Error(`${file}: not a 64-byte key`);
  return b58encode(Uint8Array.from(a));
}

export class DesktopSwarm extends Swarm {
  ctl!: Controller;
  override async send(w: Wallet, body: Body): Promise<string> {
    const da = this.ctl?.byAddress.get(w.address);
    if (!da || !["Post", "React", "Follow"].includes(body.kind)) return super.send(w, body);
    return this.ctl.viaDesktop(da, body);
  }
  override async act(a: Agent, posts: any[], mention?: any): Promise<string> {
    try {
      return await super.act(a, posts, mention);
    } catch (e) {
      if (e instanceof DeskFailed) {
        this.log("desk_failed", { agent: a.persona.handle, error: e.message.slice(0, 200) });
        return "desk_failed";
      }
      throw e;
    }
  }
}

export interface CtlOpts { roles: Role[]; agentsN: number; parallel: number; browseGapS: number; workGapS: number; tipShare: number; tipCadre: string; tipUnits: bigint; tipsPerDay: number; walletTgz: string; passwordFor: (h: string) => string }

export class Controller {
  agents: DeskAgent[] = [];
  byAddress = new Map<string, DeskAgent>();
  byHandle = new Map<string, DeskAgent>();
  swarm: DesktopSwarm;
  ledger: Ledger;
  usdPerS: number;
  stopped = false;
  capped = false;
  started = now();
  lastSpendTick = now();
  visionInFlight = 0;

  cfg: DeskConfig;
  private chat: Chat;
  private o: CtlOpts;
  constructor(
    cfg: DeskConfig,
    swarmCfg: SwarmConfig,
    deps: Deps,
    operator: Wallet,
    e2b: E2BApi,
    chat: Chat,
    o: { roles: Role[]; agentsN: number; parallel: number; browseGapS: number; workGapS: number; tipShare: number; tipCadre: string; tipUnits: bigint; tipsPerDay: number; walletTgz: string; passwordFor: (h: string) => string },
  ) {
    this.cfg = cfg; this.chat = chat; this.o = o;
    this.usdPerS = usdPerSecond(cfg.vcpu, cfg.ramGib);
    this.ledger = new Ledger(resolve(cfg.stateDir, "spend.json"), cfg.usdPerDay);
    const handles = cfg.handles ?? DEFAULT_HANDLES();
    const chosen = handles.slice(0, o.agentsN).map((h) => {
      const p = PERSONAS.find((x) => x.handle === h);
      if (!p) throw new Error(`unknown persona ${h}`);
      return p;
    });
    const swarmAgents: Agent[] = [];
    for (const persona of chosen) {
      const file = resolve(cfg.keysDir, `agent-${persona.handle}.json`);
      swarmAgents.push({ persona, wallet: loadWallet(file) });
    }
    const baseLog = deps.log;
    const self = this;
    const wrapped: Deps = {
      ...deps,
      log(e) {
        baseLog(e);
        self.onSwarmEvent(e);
      },
    };
    this.swarm = new DesktopSwarm(swarmCfg, wrapped, operator, swarmAgents);
    this.swarm.ctl = this;
    for (const sa of swarmAgents) {
      const da: DeskAgent = {
        persona: sa.persona,
        wallet: sa.wallet,
        secretB58: "",
        password: o.passwordFor(sa.persona.handle),
        desks: {} as Record<Role, Desk>,
        log: [],
        thoughts: [],
        current: {} as Record<Role, string>,
        lastTaskAt: {} as Record<Role, number>,
        busy: {} as Record<Role, boolean>,
        signing: false,
        tipsToday: { day: "", n: 0 },
      };
      for (const r of o.roles) {
        da.desks[r] = new Desk(sa.persona.handle, r, sa.wallet.address, e2b, {
          template: cfg.template, resolution: cfg.resolution, keepaliveS: cfg.keepaliveS, sessionMaxS: cfg.sessionMaxS,
          siteUrl: cfg.siteUrl, explorerUrl: cfg.explorerUrl, walletTgz: o.walletTgz, log: (e) => this.event(e),
        });
        da.current[r] = "starting";
        da.lastTaskAt[r] = 0;
        da.busy[r] = false;
      }
      this.agents.push(da);
      this.byAddress.set(sa.wallet.address, da);
      this.byHandle.set(sa.persona.handle, da);
    }
    mkdirSync(cfg.stateDir, { recursive: true });
  }

  event(e: Record<string, unknown>) {
    console.log(JSON.stringify({ t: iso(), ...e }));
    const h = typeof e.agent === "string" ? e.agent : null;
    const da = h ? this.byHandle.get(h) : null;
    if (da && typeof e.event === "string" && e.event.startsWith("desk_")) this.push(da, { t: iso(), desk: (e.role as Role) ?? "agent", kind: String(e.event), text: e.event === "desk_created" ? "desktop created" : e.event === "desk_connected" ? `desktop reconnected (${e.was})` : e.event === "desk_provisioned" ? "wallet set up, browser and stream started" : e.event === "desk_renew" ? "session renewed" : String(e.event) });
  }

  push(da: DeskAgent, x: LogEntry) {
    da.log.push(x);
    if (da.log.length > 300) da.log = da.log.slice(-300);
  }

  /** The swarm's own events: thoughts (paid inference receipts) and actions land in the agent's log. */
  onSwarmEvent(e: Record<string, unknown>) {
    const h = typeof e.agent === "string" ? e.agent : null;
    const da = h ? this.byHandle.get(h) : null;
    if (!da) return;
    if (e.event === "thought") {
      const r = e.receipt as any;
      da.thoughts.push({ t: String(e.t), purpose: String(e.purpose), request_id: r.request_id, rows: r.rows, text_sha256: r.text_sha256, charged_lamports: r.charged_lamports ?? null });
      if (da.thoughts.length > 100) da.thoughts = da.thoughts.slice(-100);
      this.push(da, { t: String(e.t), desk: "agent", kind: "thought", text: `paid thought (${e.purpose}), ${r.rows} rows`, ref: r.request_id });
    } else if (["post", "reply", "react", "follow", "filtered", "judged_no", "budget_refused", "desk_failed"].includes(String(e.event))) {
      this.push(da, { t: String(e.t), desk: "agent", kind: String(e.event), text: String(e.event).replace("_", " "), ref: (e.request_id as string) ?? undefined, tx: (e.tx as string) ?? undefined });
    }
  }

  runningDesks(): Desk[] {
    return this.agents.flatMap((a) => Object.values(a.desks)).filter((d) => d.state === "running");
  }

  /** E2B rate of everything running now, USD/s. */
  runningUsdPerS() { return this.runningDesks().length * this.usdPerS; }

  /** Books the E2B time since the last call; pauses everything when the day cap is reached. */
  async spendTick() {
    const t = now();
    const dt = (t - this.lastSpendTick) / 1000;
    this.lastSpendTick = t;
    for (const a of this.agents) for (const d of Object.values(a.desks)) if (d.state === "running") this.ledger.e2b(a.persona.handle, dt, this.usdPerS);
    const wasCapped = this.capped;
    this.capped = this.ledger.capped;
    if (this.capped && !wasCapped) {
      this.event({ event: "day_cap_reached", spent: this.ledger.spent, cap: this.cfg.usdPerDay });
      for (const d of this.runningDesks()) await d.pause();
      for (const a of this.agents) for (const r of this.o.roles) a.current[r] = `paused: day cap ${this.cfg.usdPerDay} USD reached`;
    }
    this.ledger.save();
  }

  killed() { return this.stopped || existsSync(this.cfg.killFile); }

  /** Starts (or finds) every desktop, a few at a time. */
  async ensureAll() {
    const all = this.agents.flatMap((a) => this.o.roles.map((r) => ({ a, r })));
    let i = 0;
    const worker = async () => {
      while (i < all.length && !this.killed() && !this.capped) {
        const { a, r } = all[i++]!;
        const d = a.desks[r];
        if (d.state === "running") continue;
        try {
          if (!a.secretB58) a.secretB58 = secretB58(resolve(this.cfg.keysDir, `agent-${a.persona.handle}.json`));
          await d.ensure(() => a.secretB58, () => a.password);
          a.current[r] = "idle";
        } catch (e: any) {
          d.state = "error";
          d.lastError = String(e?.message ?? e).slice(0, 200);
          a.current[r] = `desktop error: ${d.lastError}`;
          this.event({ event: "desk_error", agent: a.persona.handle, role: r, error: d.lastError });
        }
      }
    };
    await Promise.all(Array.from({ length: 4 }, worker));
  }

  async keepalive() {
    for (const d of this.runningDesks()) {
      try { await d.keepalive(); } catch (e: any) {
        this.event({ event: "desk_lost", agent: d.handle, role: d.role, error: String(e?.message ?? e).slice(0, 200) });
        d.state = "off"; d.sbx = null; d.id = null;
      }
    }
  }

  /** Runs one vision task on one desktop, booking its cost; null when the budget or a busy desk refuses it. */
  async task(da: DeskAgent, role: Role, t: Task): Promise<{ ok: boolean; summary: string } | null> {
    const d = da.desks[role];
    if (!d || d.state !== "running" || !d.sbx || da.busy[role] || this.capped || this.killed()) return null;
    const perTaskMax = 0.05;
    if (this.ledger.visionHeadroom(this.runningUsdPerS()) < perTaskMax) {
      da.current[role] = "waiting: vision budget for today is used up";
      return null;
    }
    da.busy[role] = true;
    this.visionInFlight++;
    da.current[role] = t.kind.replace("_", " ") + (t.url ? `: ${t.url.replace(/^https?:\/\//, "")}` : "");
    this.push(da, { t: iso(), desk: role, kind: "task", text: t.kind.replace("_", " ") });
    try {
      const [w, h] = this.cfg.resolution;
      const r = await runTask(d.sbx, this.chat, {
        model: this.cfg.visionModel, system: systemPrompt(da.persona.name, da.persona.handle, w, h), task: t.text, verbatim: t.verbatim, maxSteps: t.signs ? this.cfg.maxStepsPerTask + 8 : this.cfg.maxStepsPerTask, maxUsd: perTaskMax,
        onStep: (s) => {
          if (s.action === "done") return; // the task_done line carries the summary
          if (s.note) da.current[role] = s.note;
          // one line per model turn: a turn's actions share its note
          const last = da.log[da.log.length - 1];
          if (last?.kind === "step" && last.desk === role && s.note && last.text.endsWith(s.note)) last.text = `${last.text.split(":")[0]}, ${s.action}: ${s.note}`;
          else this.push(da, { t: s.t, desk: role, kind: "step", text: s.note ? `${s.action}: ${s.note}` : s.action });
        },
      });
      this.ledger.vision(da.persona.handle, r);
      this.push(da, { t: iso(), desk: role, kind: "task_done", text: r.summary || (r.ok ? "done" : "not done"), ok: r.ok });
      da.current[role] = r.summary ? `${r.ok ? "" : "gave up: "}${r.summary}` : "idle";
      this.event({ event: "task", agent: da.persona.handle, role, kind: t.kind, ok: r.ok, steps: r.steps, usd: Number(r.usd.toFixed(6)), in_tokens: r.inTokens, out_tokens: r.outTokens, error: r.error });
      return { ok: r.ok, summary: r.summary };
    } catch (e: any) {
      const msg = String(e?.message ?? e).slice(0, 200);
      this.push(da, { t: iso(), desk: role, kind: "error", text: msg });
      this.event({ event: "task_error", agent: da.persona.handle, role, kind: t.kind, error: msg });
      return { ok: false, summary: msg };
    } finally {
      da.busy[role] = false;
      da.lastTaskAt[role] = now();
      this.visionInFlight--;
    }
  }

  /** A chain action by the agent's social desktop; resolves to the tx hash it produced. */
  async viaDesktop(da: DeskAgent, body: Body): Promise<string> {
    let b = body;
    const day = iso().slice(0, 10);
    if (da.tipsToday.day !== day) da.tipsToday = { day, n: 0 };
    if (b.kind === "React" && b.items[0]?.reaction === REACTION.like && da.tipsToday.n < this.o.tipsPerDay && Math.random() < this.o.tipShare) {
      b = { kind: "React", items: [{ ...b.items[0]!, reaction: REACTION.tip, tip: this.o.tipUnits }] };
      da.tipsToday.n++;
    }
    const t = chainTask(this.cfg.siteUrl, b, this.o.tipCadre);
    if (!t) throw new DeskFailed(`no desktop task for ${b.kind}`);
    // wait for the social desktop to be free (a browse task may be running)
    for (let i = 0; i < 90 && da.busy.social; i++) await sleep(1000);
    while (da.signing) await sleep(500);
    da.signing = true;
    try {
      const before = BigInt((await this.swarm.d.node.call("getAccount", { account: da.wallet.address }))?.nonce ?? 0);
      const r = await this.task(da, "social", t);
      if (!r) throw new DeskFailed("social desktop not available (budget, cap or not running)");
      const h = await this.findTx(da.wallet.address, before, 150_000);
      if (!h) throw new DeskFailed(`no ${t.kind} tx from the desktop (${r.summary || "no summary"})`);
      this.push(da, { t: iso(), desk: "social", kind: "tx", text: `${t.kind} signed in the Cadre Wallet`, tx: h, ok: true });
      return h;
    } finally {
      da.signing = false;
    }
  }

  /** The agent's tx with nonce `nonce`: from the mempool while pending, else from its tx list. */
  async findTx(address: string, nonce: bigint, waitMs: number): Promise<string | null> {
    const node = this.swarm.d.node;
    const end = now() + waitMs;
    while (now() < end) {
      const txs = await node.call("getAccountTxs", { account: address, limit: 5 }).catch(() => null);
      const hit = txs?.txs?.find((x: any) => x.from === address && BigInt(x.nonce) === nonce);
      if (hit) return hit.tx_hash;
      const mp = await node.call("getMempool", {}).catch(() => null);
      if (mp && Number(mp.size) <= 300) {
        for (const h of mp.txs ?? []) {
          const x = await node.call("getTx", { tx_hash: h }).catch(() => null);
          if (x?.from === address && BigInt(x.nonce) === nonce) return h;
        }
      }
      await sleep(5000);
    }
    return null;
  }

  /** Read-only tasks: browsing on the social desktop, work on the work desktop, when due. */
  async background() {
    if (this.capped || this.killed()) return;
    const due: { a: DeskAgent; r: Role; t: Task }[] = [];
    for (const [i, a] of this.agents.entries()) {
      for (const r of this.o.roles) {
        const gap = (r === "work" ? this.o.workGapS : this.o.browseGapS) * 1000;
        if (a.busy[r] || a.signing || now() - a.lastTaskAt[r] < gap) continue;
        const pick = Math.floor(now() / gap) + i;
        const others = this.agents.filter((x) => x !== a).map((x) => ({ handle: x.persona.handle }));
        due.push({ a, r, t: r === "work" ? workTask(this.cfg.siteUrl, this.cfg.explorerUrl, a.wallet.address, pick) : browseTask(this.cfg.siteUrl, pick, others) });
      }
    }
    due.sort((x, y) => x.a.lastTaskAt[x.r] - y.a.lastTaskAt[y.r]);
    for (const x of due) {
      if (this.visionInFlight >= this.o.parallel) break;
      void this.task(x.a, x.r, x.t);
    }
  }

  async stopAll(kill: boolean) {
    this.stopped = true;
    for (const a of this.agents) for (const d of Object.values(a.desks)) await (kill ? d.kill() : d.pause());
  }

  status(): Record<string, unknown> {
    const m = this.ledger.measured();
    return {
      ok: !this.stopped,
      stopped: this.killed(),
      capped: this.capped,
      cap_usd_per_day: this.cfg.usdPerDay,
      spend_today: { day: this.ledger.d.day, e2b_usd: round(this.ledger.d.e2b_usd), vision_usd: round(this.ledger.d.vision_usd), total_usd: round(this.ledger.spent) },
      measured: { ...m, usd: round(m.usd), vision_usd: round(m.vision_usd), usd_per_desktop_hour: m.usd_per_desktop_hour === null ? null : round(m.usd_per_desktop_hour), usd_per_agent_hour: m.usd_per_desktop_hour === null ? null : round(m.usd_per_desktop_hour * this.o.roles.length) },
      e2b: { template: this.cfg.template, vcpu: this.cfg.vcpu, ram_gib: this.cfg.ramGib, usd_per_desktop_hour_list: round(this.usdPerS * 3600) },
      vision_model: this.cfg.visionModel,
      desktops_running: this.runningDesks().length,
      roles: this.o.roles,
      agents: this.agents.map((a) => this.agentCard(a)),
      personas_waiting: (this.cfg.handles ?? DEFAULT_HANDLES()).slice(this.o.agentsN),
    };
  }

  agentCard(a: DeskAgent) {
    const last = [...a.thoughts].reverse().find((t) => t.purpose === "post" || t.purpose === "reply") ?? a.thoughts[a.thoughts.length - 1] ?? null;
    const sp = this.ledger.d.agents[a.persona.handle];
    return {
      handle: a.persona.handle,
      name: a.persona.name,
      archetype: a.persona.archetype,
      persona: a.persona.persona,
      appearance: a.persona.appearance,
      account: a.wallet.address,
      desktops: Object.fromEntries(this.o.roles.map((r) => [r, { state: a.desks[r].state, current: a.current[r], busy: a.busy[r], error: a.desks[r].lastError }])),
      latest_thought: last,
      counters: this.swarm.st.agents[a.persona.handle]?.counters ?? {},
      spend_today: sp ? { e2b_usd: round(sp.e2b_usd), vision_usd: round(sp.vision_usd), tasks: sp.tasks } : null,
    };
  }

  detail(handle: string) {
    const a = this.byHandle.get(handle);
    if (!a) return null;
    return { ...this.agentCard(a), log: [...a.log].reverse().slice(0, 200), thoughts: [...a.thoughts].reverse() };
  }
}

const round = (x: number) => Math.round(x * 1e6) / 1e6;
