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
//
// Phase A (docs/DESKTOPS-AT-SCALE.md): agents past the first DESK_TWO_DESK_AGENTS have ONE desktop
// (role "social", the signer) that a mode scheduler switches between Social mode (town, profiles,
// leaderboards) and Work mode (wallet, explorer, market, jobs) every DESK_MODE_SLOT_S, staggered so
// about half the agents are in each mode at any time. A chain action always runs on that desktop,
// whatever the mode (it is the agent's only signer). Where a desktop runs is per agent: E2B, or our
// own boxes for the handles in DESK_SELFHOST_AGENTS (src/selfhost.ts, the same DesktopHost interface).
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
import { Desk, type DeskHealth, type DesktopHost, type Role } from "./desk.ts";
import { Ledger } from "./spend.ts";
import { browseTask, chainTask, type Task, workTask } from "./tasks.ts";
import { type Chat, runTask, systemPrompt } from "./vision.ts";

export class DeskFailed extends Error {}

/** Desktop agents are taken from the END of the house persona list (persona 30 first), so the
 * headless cadre-swarm keeps the first 30 - N personas with SWARM_AGENTS=30-N and no agent is
 * ever driven by both. */
export const DEFAULT_HANDLES = () => PERSONAS.slice().reverse().map((p) => p.handle);

export type Mode = "social" | "work";

/** Phase A mode of a one-desktop agent at time `t`: slots of `slotMs`, alternating, agent i offset by i. */
export function modeAt(index: number, t: number, slotMs: number): Mode {
  return (Math.floor(t / slotMs) + index) % 2 === 0 ? "social" : "work";
}

/** What a desktop is doing, for the site's filters (posting now, trading now, mining and jobs). */
export function activityOf(kind: string | null, busy: boolean, state: string): "posting" | "trading" | "mining" | "browsing" | "working" | "idle" | "off" {
  if (state !== "running") return "off";
  if (!busy || !kind) return "idle";
  if (["post", "reply", "like", "tip", "follow"].includes(kind)) return "posting";
  if (kind === "market") return "trading";
  if (kind === "jobs" || kind === "explorer_blocks") return "mining";
  if (kind.startsWith("read_")) return "browsing";
  return "working";
}

export interface LogEntry { t: string; desk: Role | "agent"; kind: string; text: string; ref?: string; tx?: string; ok?: boolean }
export interface ThoughtEntry { t: string; purpose: string; request_id: string; rows: number; text_sha256: string; charged_lamports: string | null }

export interface DeskAgent {
  persona: Persona;
  wallet: Wallet;
  secretB58: string;
  password: string;
  desks: Record<Role, Desk>;
  /** the desktops this agent has: ["social", "work"] (two) or ["social"] (one, mode scheduler) */
  roles: Role[];
  solo: boolean;
  index: number;
  mode: Mode;
  modeSince: number;
  taskKind: Record<Role, string | null>;
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

export interface CtlOpts {
  roles: Role[]; agentsN: number; parallel: number; browseGapS: number; workGapS: number; tipShare: number; tipCadre: string; tipUnits: bigint; tipsPerDay: number; walletTgz: string; passwordFor: (h: string) => string;
  /** how many agents (from the front of the handle list) keep every role in `roles`; the rest get one desktop (default: all keep every role) */
  twoDeskAgents?: number;
  /** read-only task gap of a one-desktop agent (default browseGapS) */
  soloGapS?: number;
  /** Social/Work slot length of a one-desktop agent (default 1800 s) */
  modeSlotS?: number;
  /** self-hosted desktop host and the agents that run on it (the rest run on E2B) */
  selfhost?: DesktopHost;
  selfHostHandles?: string[];
  /** seconds between health checks of each desktop (default 300) */
  healthS?: number;
}

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
    e2b: DesktopHost,
    chat: Chat,
    o: CtlOpts,
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
    const two = o.twoDeskAgents ?? swarmAgents.length;
    const selfSet = new Set(o.selfHostHandles ?? []);
    if (selfSet.size && !o.selfhost) throw new Error("DESK_SELFHOST_AGENTS is set but no self-hosted desktop host is configured");
    for (const [index, sa] of swarmAgents.entries()) {
      const solo = index >= two || o.roles.length < 2;
      const roles: Role[] = index >= two ? ["social"] : o.roles;
      const host = selfSet.has(sa.persona.handle) ? o.selfhost! : e2b;
      const da: DeskAgent = {
        persona: sa.persona,
        wallet: sa.wallet,
        secretB58: "",
        password: o.passwordFor(sa.persona.handle),
        desks: {} as Record<Role, Desk>,
        roles,
        solo,
        index,
        mode: solo ? modeAt(index, now(), (o.modeSlotS ?? 1800) * 1000) : "social",
        modeSince: now(),
        taskKind: {} as Record<Role, string | null>,
        log: [],
        thoughts: [],
        current: {} as Record<Role, string>,
        lastTaskAt: {} as Record<Role, number>,
        busy: {} as Record<Role, boolean>,
        signing: false,
        tipsToday: { day: "", n: 0 },
      };
      for (const r of roles) {
        da.desks[r] = new Desk(sa.persona.handle, r, sa.wallet.address, host, {
          template: cfg.template, resolution: cfg.resolution, keepaliveS: cfg.keepaliveS, sessionMaxS: cfg.sessionMaxS,
          siteUrl: cfg.siteUrl, explorerUrl: cfg.explorerUrl, walletTgz: o.walletTgz, stream: cfg.stream, log: (e) => this.event(e),
        });
        da.current[r] = "starting";
        da.lastTaskAt[r] = 0;
        da.busy[r] = false;
        da.taskKind[r] = null;
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

  /** A desktop's rate, USD/s: its host's own (self-hosted: box price / desktops) or E2B's list rate. */
  rateOf(d: Desk) { return d.api.usdPerS ?? this.usdPerS; }

  /** Rate of everything running now, USD/s. */
  runningUsdPerS() { return this.runningDesks().reduce((s, d) => s + this.rateOf(d), 0); }

  /** Books the E2B time since the last call; pauses everything when the day cap is reached. */
  async spendTick() {
    const t = now();
    const dt = (t - this.lastSpendTick) / 1000;
    this.lastSpendTick = t;
    for (const a of this.agents) for (const d of Object.values(a.desks)) if (d.state === "running") this.ledger.e2b(a.persona.handle, dt, this.rateOf(d));
    const wasCapped = this.capped;
    this.capped = this.ledger.capped;
    if (this.capped && !wasCapped) {
      this.event({ event: "day_cap_reached", spent: this.ledger.spent, cap: this.cfg.usdPerDay });
      for (const d of this.runningDesks()) await d.pause();
      for (const a of this.agents) for (const r of a.roles) a.current[r] = `paused: day cap ${this.cfg.usdPerDay} USD reached`;
    }
    this.ledger.save();
  }

  killed() { return this.stopped || existsSync(this.cfg.killFile); }

  /** Starts (or finds) every desktop, a few at a time. */
  async ensureAll() {
    const all = this.agents.flatMap((a) => a.roles.map((r) => ({ a, r })));
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
          d.flags = {};
          await d.setFlag("console", this.showsConsole(a, r)).catch(() => {});
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

  /** The activity console shows (between vision tasks) on Work desktops and on one-desktop agents in Work mode. */
  showsConsole(a: DeskAgent, r: Role): boolean { return r === "work" || (a.solo && a.mode === "work"); }

  /** A runtime event (a trade by the fast executor, a tx, a note) for an agent's log and activity console. */
  activity(handle: string, e: { kind: string; text: string; tx?: string; ref?: string }): boolean {
    const a = this.byHandle.get(handle);
    if (!a) return false;
    this.push(a, { t: iso(), desk: "agent", kind: e.kind, text: e.text, tx: e.tx, ref: e.ref, ok: true });
    return true;
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
    da.taskKind[role] = t.kind;
    d.lastActionAt = now();
    this.visionInFlight++;
    da.current[role] = t.kind.replace("_", " ") + (t.url ? `: ${t.url.replace(/^https?:\/\//, "")}` : "");
    this.push(da, { t: iso(), desk: role, kind: "task", text: t.kind.replace("_", " ") });
    try {
      // the browser host takes the activity console out of the front before the model sees the screen
      await d.setFlag("busy", true).catch(() => {});
      if (d.flags.console) await sleep(1500);
      const [w, h] = this.cfg.resolution;
      const r = await runTask(d.sbx, this.chat, {
        model: this.cfg.visionModel, system: systemPrompt(da.persona.name, da.persona.handle, w, h), task: t.text, verbatim: t.verbatim, openUrl: t.url, maxSteps: t.signs ? this.cfg.maxStepsPerTask + 8 : this.cfg.maxStepsPerTask, maxUsd: perTaskMax,
        onStep: (s) => {
          d.lastActionAt = now();
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
      await d.setFlag("busy", false).catch(() => {});
      da.busy[role] = false;
      da.taskKind[role] = null;
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
    if (da.solo && da.mode === "work") this.push(da, { t: iso(), desk: "social", kind: "mode", text: `Work mode paused for a ${t.kind} (the agent's only signing desktop)` });
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

  /** Phase A: moves each one-desktop agent to its scheduled mode (logged when it changes). */
  scheduleModes(t = now()) {
    const slot = (this.o.modeSlotS ?? 1800) * 1000;
    for (const a of this.agents) {
      if (!a.solo) continue;
      const m = modeAt(a.index, t, slot);
      if (m === a.mode) continue;
      a.mode = m;
      a.modeSince = t;
      void a.desks.social?.setFlag("console", this.showsConsole(a, "social")).catch(() => {});
      this.push(a, { t: new Date(t).toISOString(), desk: "social", kind: "mode", text: m === "work" ? "Work mode: wallet, explorer, market, jobs" : "Social mode: town, profiles, leaderboards" });
    }
  }

  /** Read-only tasks: browsing on the social desktop, work on the work desktop (or, on a one-desktop
   * agent, whichever its mode says), when due. */
  async background() {
    if (this.capped || this.killed()) return;
    this.scheduleModes();
    const due: { a: DeskAgent; r: Role; t: Task }[] = [];
    for (const [i, a] of this.agents.entries()) {
      for (const r of a.roles) {
        const mode: Mode = a.solo ? a.mode : r;
        const gap = (a.solo ? this.o.soloGapS ?? this.o.browseGapS : r === "work" ? this.o.workGapS : this.o.browseGapS) * 1000;
        if (a.busy[r] || a.signing || now() - a.lastTaskAt[r] < gap) continue;
        const pick = Math.floor(now() / gap) + i;
        const others = this.agents.filter((x) => x !== a).map((x) => ({ handle: x.persona.handle }));
        const t = mode === "work"
          ? workTask(this.cfg.siteUrl, this.cfg.explorerUrl, a.wallet.address, pick, a.solo ? { walletUrl: a.desks[r].walletUrl() } : undefined)
          : browseTask(this.cfg.siteUrl, pick, others);
        due.push({ a, r, t });
      }
    }
    due.sort((x, y) => x.a.lastTaskAt[x.r] - y.a.lastTaskAt[y.r]);
    for (const x of due) {
      if (this.visionInFlight >= this.o.parallel) break;
      void this.task(x.a, x.r, x.t);
    }
  }

  /** Health of every running desktop (stream, browser host, memory, last action); problems go to the
   * journal as desk_unhealthy (the ops log, scripts/devnet-health.sh reads /health) and the encoder or
   * browser host is restarted when it is the one that died. */
  async healthTick(): Promise<void> {
    const staleMs = 3 * Math.max(this.o.browseGapS, this.o.workGapS, this.o.soloGapS ?? 0) * 1000;
    const desks = this.agents.flatMap((a) => a.roles.map((r) => ({ a, r, d: a.desks[r] }))).filter((x) => x.d.state === "running");
    let i = 0;
    const worker = async () => {
      while (i < desks.length && !this.killed()) {
        const { a, r, d } = desks[i++]!;
        try {
          const h = await d.checkHealth();
          const issues = deskIssues(h, d.lastActionAt, staleMs, now(), this.started);
          if (issues.length) {
            const healed = await d.heal().catch(() => [] as string[]);
            this.event({ event: "desk_unhealthy", agent: a.persona.handle, role: r, issues, healed, mem_avail_mb: h?.mem_avail_mb ?? null, stream_age_s: h?.stream_age_s ?? null });
          }
        } catch (e: any) {
          this.event({ event: "desk_unhealthy", agent: a.persona.handle, role: r, issues: [`health check failed: ${String(e?.message ?? e).slice(0, 120)}`] });
        }
      }
    };
    await Promise.all(Array.from({ length: 6 }, worker));
  }

  /** Summary for /health and the ops log: one entry per desktop with its problems. */
  healthSummary() {
    const staleMs = 3 * Math.max(this.o.browseGapS, this.o.workGapS, this.o.soloGapS ?? 0) * 1000;
    const rows = this.agents.flatMap((a) => a.roles.map((r) => {
      const d = a.desks[r];
      const issues = d.state === "running" ? deskIssues(d.health, d.lastActionAt, staleMs, now(), this.started) : [`desktop ${d.state}`];
      return { agent: a.persona.handle, role: r, host: d.api.kind ?? "e2b", state: d.state, issues };
    }));
    return { desktops: rows.length, running: rows.filter((x) => x.state === "running").length, streams_alive: this.agents.reduce((s, a) => s + a.roles.filter((r) => a.desks[r].health?.stream_alive).length, 0), unhealthy: rows.filter((x) => x.issues.length) };
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
      measured: { ...m, usd: round(m.usd), vision_usd: round(m.vision_usd), usd_per_desktop_hour: m.usd_per_desktop_hour === null ? null : round(m.usd_per_desktop_hour), usd_per_agent_hour: m.usd_per_desktop_hour === null || !this.agents.length ? null : round((m.usd_per_desktop_hour * this.agents.reduce((s, x) => s + x.roles.length, 0)) / this.agents.length) },
      e2b: { template: this.cfg.template, vcpu: this.cfg.vcpu, ram_gib: this.cfg.ramGib, usd_per_desktop_hour_list: round(this.usdPerS * 3600) },
      vision_model: this.cfg.visionModel,
      stream: { fps: Number(this.cfg.stream.DESK_FPS ?? 4), kbps: Number(this.cfg.stream.DESK_KBPS ?? 350), seg_s: Number(this.cfg.stream.DESK_SEG_S ?? 2), still_s: Number(this.cfg.stream.DESK_STILL_S ?? 2) },
      desktops_running: this.runningDesks().length,
      desktops_total: this.agents.reduce((s, x) => s + x.roles.length, 0),
      roles: this.o.roles,
      layout: { two_desktops: this.agents.filter((a) => !a.solo).length, one_desktop: this.agents.filter((a) => a.solo).length, mode_slot_s: this.o.modeSlotS ?? 1800 },
      health: this.healthSummary(),
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
      layout: a.solo ? "one_desktop" : "two_desktops",
      mode: a.solo ? a.mode : null,
      mode_since: a.solo ? new Date(a.modeSince).toISOString() : null,
      desktops: Object.fromEntries(a.roles.map((r) => {
        const d = a.desks[r];
        return [r, {
          state: d.state, current: a.current[r], busy: a.busy[r], error: d.lastError, host: d.api.kind ?? "e2b",
          task_kind: a.taskKind[r], activity: activityOf(a.taskKind[r], a.busy[r], d.state),
          last_action_at: d.lastActionAt ? new Date(d.lastActionAt).toISOString() : null,
          health: d.health,
        }];
      })),
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

/** The problems a desktop's last health check shows (empty = healthy). A desktop with no action in
 * `staleMs` (3 task gaps) is flagged once the controller has run that long. */
export function deskIssues(h: DeskHealth | null, lastActionAt: number, staleMs: number, t: number, started: number): string[] {
  const out: string[] = [];
  if (!h) return []; // not checked yet (the first check runs a minute after start)
  if (!h.encoder) out.push("stream encoder down");
  else if (!h.stream_alive) out.push(`stream stale (${h.stream_age_s ?? "no"} s)`);
  if (!h.browser_host) out.push("browser host down");
  if (h.mem_avail_mb !== null && h.mem_avail_mb < 80) out.push(`memory low (${h.mem_avail_mb} MB free)`);
  if (t - started > staleMs && t - lastActionAt > staleMs) out.push(lastActionAt ? `no action for ${Math.round((t - lastActionAt) / 60000)} min` : "no action since the controller started");
  return out;
}
