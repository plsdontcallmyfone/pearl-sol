// The house desktops controller. Since 2026-10-23 (owner: "none of these agents are doing any actual
// work"; "not social and not work things, they're both for work") every house agent has a job
// (src/roster.ts) and every desktop it has does that job: research briefs sold on the market,
// source reviews with verdicts, QA bug lists, monitor reports, practice trades, a digest. The work
// loop is src/work.ts; scripted browser steps run in the desktop's browser host without a model, and
// the vision model is used only where reading or judging a page needs it (research, flow checks).
//
// The desktops keep their stable role ids ("social" = desktop 1, "work" = desktop 2): they are in
// every sandbox's metadata and stream token. There is no Social/Work mode any more, no town browsing
// and no town posting; the old town behaviour (the swarm's posts typed on a desktop) runs only with
// DESK_TOWN=1. The wallet in each desktop stays locked: no work task signs in the browser; market
// calls are signed by this controller with the agent's own key.
import { addCx } from "../../../../apps/explorer/src/cxjson.ts";
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
import { Desk, DESK_NUMBER, deskLabel, type DeskHealth, type DesktopHost, type Role, type StepResult, type Visit } from "./desk.ts";
import { CreditGate, probeCredits, type ProbeResult } from "./credits.ts";
import { daySummary, Ledger } from "./spend.ts";
import { chainTask, type Task } from "./tasks.ts";
import { type MarketOps } from "./market.ts";
import { ReportStore } from "./reports.ts";
import { jobOf, jobText } from "./roster.ts";
import { type WorkAgent, type WorkCfg, workDefaults, WorkEngine, type WorkHost } from "./work.ts";
import { isTownSolana, type TownNode } from "../../src/townnode.ts";
import { type Chat, runTask, systemPrompt, type TaskResult, workSystemPrompt } from "./vision.ts";

export class DeskFailed extends Error {}

/** Desktop agents are taken from the END of the house persona list (persona 30 first), so the
 * headless cadre-swarm keeps the first 30 - N personas with SWARM_AGENTS=30-N and no agent is
 * ever driven by both. */
export const DEFAULT_HANDLES = () => PERSONAS.slice().reverse().map((p) => p.handle);

/** What a desktop is doing, for the site's filters. Work kinds: research, qa_*, monitor_show, check_sources,
 * read_story, hire_page, trade_page, strategy_page, digest_show; the old town kinds only with DESK_TOWN=1. */
export function activityOf(kind: string | null, busy: boolean, state: string): "posting" | "trading" | "mining" | "researching" | "testing" | "monitoring" | "reviewing" | "hiring" | "browsing" | "working" | "idle" | "off" {
  if (state !== "running") return "off";
  if (!busy || !kind) return "idle";
  if (["post", "reply", "like", "tip", "follow"].includes(kind)) return "posting";
  if (kind === "research") return "researching";
  if (kind.startsWith("qa_")) return "testing";
  if (kind === "monitor_show") return "monitoring";
  if (kind === "check_sources") return "reviewing";
  if (kind === "read_story" || kind === "hire_page") return "hiring";
  if (kind === "trade_page" || kind === "strategy_page" || kind === "market") return "trading";
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
  /** the desktops this agent has: ["social", "work"] (desktops 1 and 2) or ["social"] (desktop 1) */
  roles: Role[];
  solo: boolean;
  index: number;
  /** since when each desktop has been busy (0 = free), for the stuck check */
  busySince: Record<Role, number>;
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
    // the vision model is out of credit: a post, reply or reaction would pay for a Cadre thought that
    // no desktop can type, so the agent waits (its DMs, answered off the desktops, still run)
    if (this.ctl?.credits.paused) { this.ctl.credits.skip(); return "vision_paused"; }
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
  /** memory under this (MB available) is a health problem; the desktop's tabs are trimmed, then its
   * browser restarted if the next check is still under it (default 80) */
  memLowMb?: number;
  /** the vision model's credit gate (src/credits.ts); defaults in creditDefaults */
  credit?: Partial<typeof creditDefaults>;
  /** GET /api/v1/key for the credit probe (absent: the probe only sends its tiny chat request) */
  keyInfo?: () => Promise<{ status: number; json: any }>;
  /** the work loop's outside reads and the market (src/work.ts); absent: no job runs (tests of the desktops alone) */
  work?: {
    market: MarketOps | null;
    api: (path: string) => Promise<any>;
    fetchPage: WorkHost["fetchPage"];
    fetchRaw: WorkHost["fetchRaw"];
    solBalances?: WorkHost["solBalances"];
    practice?: (handle: string) => any | null;
    publicApi?: string;
    cfg?: Partial<WorkCfg>;
  };
  /** DESK_TOWN=1: the swarm's town actions are typed on desktop 1 as before 2026-10-23 (default off) */
  town?: boolean;
}

/** Credit gate defaults: probe every 5 min (doubling to 1 h when a resume does not hold for 10 min),
 * pause the desktops themselves after 30 min without credit, a "still paused" line every 30 min. */
export const creditDefaults = { probeS: 300, flapS: 600, maxProbeS: 3600, holdDesksS: 1800, remindS: 1800, minUsd: 0.01, probeMaxTokens: 1600 };

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
  lastSpendLog = 0;
  visionInFlight = 0;
  credits: CreditGate;
  creditCfg: typeof creditDefaults;

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
    // one spend_day line per finished UTC day: E2B, vision (tasks + probes), total against the cap
    this.ledger = new Ledger(resolve(cfg.stateDir, "spend.json"), cfg.usdPerDay, undefined, (d) => this.event({ event: "spend_day", ...daySummary(d, cfg.usdPerDay) }));
    this.creditCfg = { ...creditDefaults, ...(o.credit ?? {}) };
    this.credits = new CreditGate({ ...this.creditCfg, log: (e) => this.event(e) });
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
        busySince: {} as Record<Role, number>,
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
          template: r === "work" && cfg.workTemplate ? cfg.workTemplate : cfg.template, resolution: cfg.resolution, keepaliveS: cfg.keepaliveS, sessionMaxS: cfg.sessionMaxS,
          sizeTag: r === "work" && cfg.workRamGib ? `${Math.round(cfg.workRamGib * 1024)}` : undefined,
          usdPerS: r === "work" && cfg.workRamGib ? usdPerSecond(cfg.vcpu, cfg.workRamGib) : undefined,
          siteUrl: cfg.siteUrl, explorerUrl: cfg.explorerUrl, walletTgz: o.walletTgz, stream: cfg.stream, log: (e) => this.event(e),
        });
        da.current[r] = "starting";
        da.lastTaskAt[r] = 0;
        da.busy[r] = false;
        da.busySince[r] = 0;
        da.taskKind[r] = null;
      }
      this.agents.push(da);
      this.byAddress.set(sa.wallet.address, da);
      this.byHandle.set(sa.persona.handle, da);
    }
    mkdirSync(cfg.stateDir, { recursive: true });
    this.store = new ReportStore(resolve(cfg.stateDir, "reports"));
    if (o.work) {
      const wa: WorkAgent[] = this.agents.flatMap((a) => { const j = jobOf(a.persona.handle); return j ? [{ handle: a.persona.handle, name: a.persona.name, wallet: a.wallet, roles: a.roles, job: j }] : []; });
      this.work = new WorkEngine(this.host(), wa, this.store, { ...workDefaults(resolve(cfg.stateDir, "work-state.json")), ...(o.work.cfg ?? {}) });
    }
  }

  store: ReportStore;
  work: WorkEngine | null = null;

  /** The work loop's view of this controller (src/work.ts WorkHost). */
  host(): WorkHost {
    const w = this.o.work!;
    const ctl = this;
    return {
      site: this.cfg.siteUrl,
      explorer: this.cfg.explorerUrl,
      publicApi: w.publicApi ?? this.cfg.siteUrl.replace("//cadre.", "//api."),
      now,
      event: (e) => this.event(e),
      note: (h, kind, text, x) => { const a = this.byHandle.get(h); if (a) this.push(a, { t: iso(), desk: "agent", kind, text: text.slice(0, 300), tx: x?.tx, ref: x?.ref, ok: x?.ok }); },
      freeRoles: (h) => { const a = this.byHandle.get(h); return a && !this.capped && !this.killed() && !this.credits.desksHeld ? a.roles.filter((r) => a.desks[r].state === "running" && a.desks[r].sbx && !a.busy[r] && !(a.signing && r === "social")) : []; },
      script: (h, r, kind, steps, x) => this.scriptOn(h, r, kind, steps, x),
      vision: (h, r, t) => { const a = this.byHandle.get(h); return a ? this.task(a, r, { kind: t.kind, signs: false, url: t.url, text: t.text, work: true, report: t.report, maxSteps: t.maxSteps, maxUsd: t.maxUsd, record: t.record }) as any : Promise.resolve(null); },
      visionReady: () => !this.credits.paused && !this.capped && !this.killed() && this.ledger.visionHeadroom(this.runningUsdPerS()) >= 0.05,
      api: w.api,
      node: (m, p) => this.swarm.d.node.call(m, p ?? {}),
      fetchPage: w.fetchPage,
      fetchRaw: w.fetchRaw,
      solBalances: w.solBalances,
      self: () => ({ spent: ctl.ledger.spent, cap: ctl.cfg.usdPerDay, e2b: ctl.ledger.d.e2b_usd, vision: ctl.ledger.d.vision_usd, unhealthy: ctl.healthSummary().unhealthy.length, desktops: ctl.agents.reduce((n, a) => n + a.roles.length, 0), running: ctl.runningDesks().length, vision_paused: ctl.credits.paused, seconds_left: ctl.ledger.secondsLeft(), running_usd_per_s: ctl.runningUsdPerS() }),
      practice: (h) => w.practice?.(h) ?? null,
      fund: async (h, units) => {
        const a = this.byHandle.get(h);
        if (!a) return null;
        try { return await this.swarm.send(this.swarm.op, { kind: "Transfer", to: a.wallet.publicKey, amount: units }); }
        catch (e: any) { this.event({ event: "work_fund_error", agent: h, error: String(e?.message ?? e).slice(0, 160) }); return null; }
      },
      market: w.market,
    };
  }

  /** Scripted browser steps on one desktop (no model), shown on its stream; logged in the agent's work log. */
  async scriptOn(h: string, r: Role, kind: string, steps: { url?: string; wait_ms?: number; highlight?: string; scroll?: number }[], x?: { collect?: boolean; viewport?: { width: number; height: number } }): Promise<StepResult[] | null> {
    const da = this.byHandle.get(h);
    const d = da?.desks[r];
    if (!da || !d || d.state !== "running" || !d.sbx || da.busy[r] || this.capped || this.killed() || !steps.length) return null;
    da.busy[r] = true;
    da.busySince[r] = now();
    da.taskKind[r] = kind;
    d.lastActionAt = now();
    da.current[r] = `${kind.replace(/_/g, " ")}: ${steps.length} page${steps.length > 1 ? "s" : ""}`;
    this.push(da, { t: iso(), desk: r, kind: "task", text: `${kind.replace(/_/g, " ")} (${steps.length} page${steps.length > 1 ? "s" : ""}, scripted)` });
    try {
      await d.setFlag("busy", true).catch(() => {});
      await d.waitWorkFront().catch(() => false);
      const res = await d.runScript(steps, x);
      d.lastActionAt = now();
      for (const [i, sr] of (res ?? []).entries()) this.push(da, { t: iso(), desk: r, kind: "step", text: `open ${String(steps[i]?.url ?? "").replace(/^https?:\/\//, "").slice(0, 90)}: ${sr.error ? `failed (${sr.error.slice(0, 60)})` : `HTTP ${sr.status ?? "?"}, ${sr.ms ?? "?"} ms${sr.found === true ? ", quote found" : sr.found === false ? ", quote not found" : ""}`}`, ok: !sr.error });
      this.event({ event: "script", agent: h, role: r, kind, steps: steps.length, ok: !!res });
      da.current[r] = res ? `${kind.replace(/_/g, " ")}: done` : `${kind.replace(/_/g, " ")}: the browser host did not answer`;
      return res;
    } finally {
      await d.setFlag("busy", false).catch(() => {});
      da.busy[r] = false;
      da.busySince[r] = 0;
      da.taskKind[r] = null;
      da.lastTaskAt[r] = now();
    }
  }

  event(e: Record<string, unknown>) {
    console.log(JSON.stringify(addCx({ t: iso(), ...e })));
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
  rateOf(d: Desk) { return d.api.usdPerS ?? d.rate ?? this.usdPerS; }

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
    // hourly: today's spend so far against the cap (the day's final line is spend_day, at 00:00 UTC)
    if (t - this.lastSpendLog >= 3600_000) {
      this.lastSpendLog = t;
      const s = daySummary(this.ledger.d, this.cfg.usdPerDay);
      const run = this.runningUsdPerS();
      this.event({ event: "spend", ...s, left_usd: round(this.cfg.usdPerDay - this.ledger.spent), running_e2b_usd_per_h: round(run * 3600), e2b_if_running_to_midnight_usd: round(s.e2b_usd + run * this.ledger.secondsLeft()), vision_paused: this.credits.paused });
    }
    this.ledger.save();
  }

  killed() { return this.stopped || existsSync(this.cfg.killFile); }

  /** Starts (or finds) every desktop, a few at a time. */
  async ensureAll() {
    const all = this.agents.flatMap((a) => a.roles.map((r) => ({ a, r })));
    let i = 0;
    const worker = async () => {
      while (i < all.length && !this.killed() && !this.holdDesks()) {
        const { a, r } = all[i++]!;
        const d = a.desks[r];
        if (d.state === "running") continue;
        try {
          if (!a.secretB58) a.secretB58 = secretB58(resolve(this.cfg.keysDir, `agent-${a.persona.handle}.json`));
          await d.ensure(() => a.secretB58, () => a.password);
          a.current[r] = "idle";
          d.flags = {};
          await d.setFlag("console", this.showsConsole(a, r)).catch(() => {});
          // no work task signs in the browser: the wallet stays locked (DESK_TOWN=1 needs it unlocked)
          await d.setFlag("wallet-locked", !this.o.town).catch(() => {});
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

  /** The activity console (the agent's work log) shows on every desktop between tasks. */
  showsConsole(_a: DeskAgent, _r: Role): boolean { return true; }

  /** A runtime event (a trade by the fast executor, a tx, a note) for an agent's log and activity console. */
  activity(handle: string, e: { kind: string; text: string; tx?: string; ref?: string }): boolean {
    const a = this.byHandle.get(handle);
    if (!a) return false;
    this.push(a, { t: iso(), desk: "agent", kind: e.kind, text: e.text, tx: e.tx, ref: e.ref, ok: true });
    return true;
  }

  /** Runs one vision task on one desktop, booking its cost; null when the budget or a busy desk refuses it. */
  async task(da: DeskAgent, role: Role, t: Task & { work?: boolean; report?: boolean; maxSteps?: number; maxUsd?: number; record?: boolean }): Promise<(Partial<TaskResult> & { ok: boolean; summary: string; visits?: Visit[] }) | null> {
    const d = da.desks[role];
    if (!d || d.state !== "running" || !d.sbx || da.busy[role] || this.capped || this.killed()) return null;
    if (this.credits.paused) {
      this.credits.skip();
      da.current[role] = "paused: vision model out of credit";
      return null;
    }
    const perTaskMax = t.maxUsd ?? 0.05;
    if (this.ledger.visionHeadroom(this.runningUsdPerS()) < perTaskMax) {
      da.current[role] = "waiting: vision budget for today is used up";
      return null;
    }
    da.busy[role] = true;
    da.busySince[role] = now();
    da.taskKind[role] = t.kind;
    d.lastActionAt = now();
    this.visionInFlight++;
    da.current[role] = t.kind.replace("_", " ") + (t.url ? `: ${t.url.replace(/^https?:\/\//, "")}` : "");
    this.push(da, { t: iso(), desk: role, kind: "task", text: t.kind.replace("_", " ") });
    try {
      // the browser host takes the activity console out of the front before the model sees the screen
      await d.setFlag("busy", true).catch(() => {});
      await d.waitWorkFront().catch(() => false);
      await sleep(400); // the tab switch settles before the first key
      if (t.record) await d.startRecording().catch(() => {});
      const [w, h] = this.cfg.resolution;
      const system = t.work ? workSystemPrompt(da.persona.name, da.persona.handle, `${jobText(da.persona.handle).title}: ${jobText(da.persona.handle).description}`, w, h) : systemPrompt(da.persona.name, da.persona.handle, w, h);
      const r = await runTask(d.sbx, this.chat, {
        model: this.cfg.visionModel, system, task: t.text, verbatim: t.verbatim, openUrl: t.url, report: t.report, maxSteps: t.maxSteps ?? (t.signs ? this.cfg.maxStepsPerTask + 8 : this.cfg.maxStepsPerTask), maxUsd: perTaskMax,
        halted: () => this.credits.paused,
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
      if (r.credit) this.credits.trip(r.credit, `${da.persona.handle}/${role} ${t.kind}`);
      this.push(da, { t: iso(), desk: role, kind: "task_done", text: r.summary || (r.ok ? "done" : "not done"), ok: r.ok });
      da.current[role] = r.credit || r.error === "vision_paused" ? "paused: vision model out of credit" : r.summary ? `${r.ok ? "" : "gave up: "}${r.summary}` : "idle";
      const visits = t.record ? await d.stopRecording().catch(() => [] as Visit[]) : undefined;
      this.event({ event: "task", agent: da.persona.handle, role, kind: t.kind, ok: r.ok, steps: r.steps, usd: Number(r.usd.toFixed(6)), in_tokens: r.inTokens, out_tokens: r.outTokens, error: r.error, report: r.report ? { findings: r.report.findings.length, problems: r.report.problems.length } : undefined, pages: visits?.length });
      return { ...r, visits };
    } catch (e: any) {
      const msg = String(e?.message ?? e).slice(0, 200);
      this.push(da, { t: iso(), desk: role, kind: "error", text: msg });
      this.event({ event: "task_error", agent: da.persona.handle, role, kind: t.kind, error: msg });
      return { ok: false, summary: msg };
    } finally {
      await d.setFlag("busy", false).catch(() => {});
      if (t.record) await d.setFlag("record", false).catch(() => {});
      da.busy[role] = false;
      da.busySince[role] = 0;
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
    const t = chainTask(this.cfg.siteUrl, b, this.o.tipCadre, isTownSolana(this.swarm.d.node) ? "SOL" : "CADRE");
    if (!t) throw new DeskFailed(`no desktop task for ${b.kind}`);
    // wait for the social desktop to be free (a browse task may be running)
    for (let i = 0; i < 90 && da.busy.social; i++) await sleep(1000);
    while (da.signing) await sleep(500);
    da.signing = true;
    try {
      const node = this.swarm.d.node;
      const sol = isTownSolana(node) ? (node as TownNode) : null;
      const startedAt = Date.now();
      const before = sol ? 0n : BigInt((await node.call("getAccount", { account: da.wallet.address }))?.nonce ?? 0);
      const r = await this.task(da, "social", t);
      if (!r) throw new DeskFailed(this.credits.paused ? "vision model paused (out of credit)" : "social desktop not available (budget, cap or not running)");
      if (this.credits.paused && !r.ok) throw new DeskFailed("vision model paused (out of credit)");
      // the town on Solana: the site signs a Solana tx in the wallet; find it by the agent's signatures
      const h = sol ? await this.findSolTx(sol, da.wallet.address, startedAt, 150_000) : await this.findTx(da.wallet.address, before, 150_000);
      if (!h) throw new DeskFailed(`no ${t.kind} tx from the desktop (${r.summary || "no summary"})`);
      this.push(da, { t: iso(), desk: "social", kind: "tx", text: `${t.kind} signed in the Cadre Wallet`, tx: h, ok: true });
      return h;
    } finally {
      da.signing = false;
    }
  }

  /** The agent's first successful town tx on Solana since `since` (ms), or null after `waitMs`. */
  async findSolTx(node: TownNode, address: string, since: number, waitMs: number): Promise<string | null> {
    const end = now() + waitMs;
    while (now() < end) {
      const l = await node.sol<any[]>("getSignaturesForAddress", [address, { limit: 10, commitment: "confirmed" }]).catch(() => []);
      const hit = l.filter((x) => !x.err && (x.blockTime ?? 0) * 1000 >= since - 5000).pop();
      if (hit) {
        const tx = await node.sol<any>("getTransaction", [hit.signature, { encoding: "json", commitment: "confirmed", maxSupportedTransactionVersion: 0 }]).catch(() => null);
        const keys: string[] = tx?.transaction?.message?.accountKeys ?? [];
        if (keys.includes(node.program)) return hit.signature;
      }
      await sleep(5000);
    }
    return null;
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

  /** The work loop (src/work.ts): every agent's due job units, started in the background. */
  async background() {
    if (this.capped || this.killed() || !this.work) return;
    await this.work.tick();
  }

  /** Desktops stay paused (not started, not resumed): the day cap, or the credit gate holding them. */
  holdDesks() { return this.capped || this.credits.desksHeld; }

  /** The credit probe (src/credits.ts) with this controller's model and key. */
  probe = (): Promise<ProbeResult> => probeCredits({ keyInfo: this.o.keyInfo, chat: this.chat, model: this.cfg.visionModel, maxTokens: this.creditCfg.probeMaxTokens, minUsd: this.creditCfg.minUsd });

  /** Every minute: while the vision model is out of credit, probe when due (booking the probe's cost),
   * and after holdDesksS pause the running desktops (E2B bills them by the second; a paused sandbox
   * bills nothing and resumes in about a second). On resume the desktops start again in the next
   * ensureAll. One pause and one resume per outage: no churn. */
  async creditTick(): Promise<void> {
    if (!this.credits.paused) {
      if (this.credits.desksHeld) {
        this.credits.desksHeld = false;
        for (const a of this.agents) for (const r of a.roles) if (a.desks[r].state !== "running") a.current[r] = "resuming";
      }
      return;
    }
    await this.credits.tick(async () => {
      const r = await this.probe();
      if (r.usd) this.ledger.probe(r.usd);
      return r;
    });
    if (this.credits.holdDue()) {
      const running = this.runningDesks();
      // a task or a signing action still running finishes first (it stops at its next model call)
      if (this.agents.some((a) => a.signing || Object.values(a.busy).some(Boolean))) return;
      this.credits.desksHeld = true;
      for (const d of running) await d.pause();
      for (const a of this.agents) for (const r of a.roles) a.current[r] = "paused: vision model out of credit (desktop paused to save E2B time)";
      this.event({ event: "vision_paused_desktops", desktops: running.length, paused_min: Math.round((now() - this.credits.since) / 60000) });
    }
    if (!this.credits.paused && this.credits.desksHeld) await this.creditTick();
  }

  /** Health of every running desktop (stream, browser host, memory, last action); problems go to the
   * journal as desk_unhealthy (the ops log, scripts/devnet-health.sh reads /health) and the encoder or
   * browser host is restarted when it is the one that died. */
  async healthTick(): Promise<void> {
    // a desktop with no action is normal now (a research desk waits for hires); stuck = busy for 20 min
    const staleMs = Number.POSITIVE_INFINITY;
    const desks = this.agents.flatMap((a) => a.roles.map((r) => ({ a, r, d: a.desks[r] }))).filter((x) => x.d.state === "running");
    const memLow = this.o.memLowMb ?? 80;
    // while the vision model is paused nothing acts on purpose: "no action" is not a desktop problem then
    const stale = this.credits.paused ? Number.POSITIVE_INFINITY : staleMs;
    let i = 0;
    const worker = async () => {
      while (i < desks.length && !this.killed()) {
        const { a, r, d } = desks[i++]!;
        try {
          const h = await d.checkHealth();
          const issues = deskIssues(h, d.lastActionAt, stale, now(), this.started, memLow);
          if (a.busySince[r] && now() - a.busySince[r] > 20 * 60_000) issues.push(`busy with one task for ${Math.round((now() - a.busySince[r]) / 60000)} min`);
          const healed = issues.length ? await d.heal(a.busy[r] || (r === "social" && a.signing), memLow).catch(() => [] as string[]) : [];
          // one line when the problems change or something was repaired, else at most every 30 min
          // (before 2026-10-23 a desktop between 60 and 80 MB logged the same line every 5 min, unrepaired)
          const key = issues.map((x) => x.replace(/\d+/g, "N")).join("|");
          if (issues.length && (healed.length || key !== d.issueKey || now() - d.issueLoggedAt >= 1800_000)) {
            d.issueLoggedAt = now();
            this.event({ event: "desk_unhealthy", agent: a.persona.handle, role: r, desktop: DESK_NUMBER[r], layout: a.solo ? "one_desktop" : "two_desktops", issues, healed, mem_avail_mb: h?.mem_avail_mb ?? null, mem_total_mb: h?.mem_total_mb ?? null, tabs: h?.tabs ?? null, chrome_mb: h?.chrome_mb ?? null, ffmpeg_mb: h?.ffmpeg_mb ?? null, stream_age_s: h?.stream_age_s ?? null });
          }
          d.issueKey = key;
        } catch (e: any) {
          this.event({ event: "desk_unhealthy", agent: a.persona.handle, role: r, issues: [`health check failed: ${String(e?.message ?? e).slice(0, 120)}`] });
        }
      }
    };
    await Promise.all(Array.from({ length: 6 }, worker));
    if (desks.length) this.event({ event: "desk_health", ...this.memorySummary(memLow) });
  }

  /** Memory of the running desktops by kind (two-desktop Social, two-desktop Work, one-desktop), from
   * the last health check: MB available (min, p10, p50), tabs, Chromium and encoder RSS (p50). Logged
   * once per health tick as desk_health, so memory is measured in the journal, not only when low. */
  memorySummary(memLow = this.o.memLowMb ?? 80) {
    const groups: Record<string, DeskHealth[]> = { two_desktops_1: [], two_desktops_2: [], one_desktop: [] };
    for (const a of this.agents) for (const r of a.roles) {
      const h = a.desks[r].health;
      if (a.desks[r].state !== "running" || !h) continue;
      groups[a.solo ? "one_desktop" : r === "work" ? "two_desktops_2" : "two_desktops_1"]!.push(h);
    }
    const q = (xs: number[], p: number) => (xs.length ? xs.slice().sort((x, y) => x - y)[Math.min(xs.length - 1, Math.floor(p * xs.length))]! : null);
    const nums = (hs: DeskHealth[], k: keyof DeskHealth) => hs.map((h) => h[k]).filter((x): x is number => typeof x === "number");
    const out: Record<string, unknown> = { mem_low_mb: memLow };
    for (const [k, hs] of Object.entries(groups)) {
      if (!hs.length) continue;
      const av = nums(hs, "mem_avail_mb");
      out[k] = { desktops: hs.length, mem_total_mb: q(nums(hs, "mem_total_mb"), 0.5), avail_min: q(av, 0), avail_p10: q(av, 0.1), avail_p50: q(av, 0.5), low: av.filter((x) => x < memLow).length, tabs_p50: q(nums(hs, "tabs"), 0.5), chrome_mb_p50: q(nums(hs, "chrome_mb"), 0.5), ffmpeg_mb_p50: q(nums(hs, "ffmpeg_mb"), 0.5) };
    }
    return out;
  }

  /** Summary for /health and the ops log: one entry per desktop with its problems. */
  healthSummary() {
    const staleMs = Number.POSITIVE_INFINITY;
    const rows = this.agents.flatMap((a) => a.roles.map((r) => {
      const d = a.desks[r];
      const issues = d.state === "running" ? deskIssues(d.health, d.lastActionAt, this.credits.paused ? Number.POSITIVE_INFINITY : staleMs, now(), this.started, this.o.memLowMb ?? 80) : this.credits.desksHeld && d.state === "paused" ? [] : [`desktop ${d.state}`];
      if (d.state === "running" && a.busySince[r] && now() - a.busySince[r] > 20 * 60_000) issues.push(`busy with one task for ${Math.round((now() - a.busySince[r]) / 60000)} min`);
      return { agent: a.persona.handle, role: r, desktop: DESK_NUMBER[r], host: d.api.kind ?? "e2b", state: d.state, issues };
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
      // the vision model's credit gate: paused = no vision task or desktop chain action starts
      vision_paused: this.credits.paused,
      vision_gate: this.credits.status(),
      cap_usd_per_day: this.cfg.usdPerDay,
      spend_today: { day: this.ledger.d.day, e2b_usd: round(this.ledger.d.e2b_usd), vision_usd: round(this.ledger.d.vision_usd), probe_usd: round(this.ledger.d.probe_usd ?? 0), total_usd: round(this.ledger.spent), within_cap: this.ledger.spent <= this.cfg.usdPerDay },
      spend_days: this.ledger.days(),
      memory: this.memorySummary(),
      measured: { ...m, usd: round(m.usd), vision_usd: round(m.vision_usd), usd_per_desktop_hour: m.usd_per_desktop_hour === null ? null : round(m.usd_per_desktop_hour), usd_per_agent_hour: m.usd_per_desktop_hour === null || !this.agents.length ? null : round((m.usd_per_desktop_hour * this.agents.reduce((s, x) => s + x.roles.length, 0)) / this.agents.length) },
      e2b: { template: this.cfg.template, vcpu: this.cfg.vcpu, ram_gib: this.cfg.ramGib, usd_per_desktop_hour_list: round(this.usdPerS * 3600) },
      vision_model: this.cfg.visionModel,
      stream: { fps: Number(this.cfg.stream.DESK_FPS ?? 4), kbps: Number(this.cfg.stream.DESK_KBPS ?? 350), seg_s: Number(this.cfg.stream.DESK_SEG_S ?? 2), still_s: Number(this.cfg.stream.DESK_STILL_S ?? 2) },
      desktops_running: this.runningDesks().length,
      desktops_total: this.agents.reduce((s, x) => s + x.roles.length, 0),
      roles: this.o.roles,
      layout: { two_desktops: this.agents.filter((a) => !a.solo).length, one_desktop: this.agents.filter((a) => a.solo).length, mode_slot_s: null },
      desktop_labels: { social: deskLabel("social"), work: deskLabel("work") },
      town: !!this.o.town,
      work: this.work ? { running_units: this.work.running.size, cadre_funded_today: this.work.st.fund } : null,
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
      job: (() => { const j = jobOf(a.persona.handle); const t = jobText(a.persona.handle); return j ? { kind: j.kind, title: t.title, description: t.description, outputs: j.outputs, target: j.target ?? j.suite ?? j.desk ?? null } : null; })(),
      account: a.wallet.address,
      layout: a.solo ? "one_desktop" : "two_desktops",
      // no Social/Work modes since 2026-10-23; kept null for readers of the old shape
      mode: null,
      mode_since: null,
      desktops: Object.fromEntries(a.roles.map((r) => {
        const d = a.desks[r];
        return [r, {
          number: DESK_NUMBER[r], label: deskLabel(r),
          state: d.state, current: a.current[r], busy: a.busy[r], error: d.lastError, host: d.api.kind ?? "e2b",
          task_kind: a.taskKind[r], activity: activityOf(a.taskKind[r], a.busy[r], d.state),
          last_action_at: d.lastActionAt ? new Date(d.lastActionAt).toISOString() : null,
          health: d.health,
        }];
      })),
      latest_thought: last,
      counters: this.swarm.st.agents[a.persona.handle]?.counters ?? {},
      spend_today: sp ? { e2b_usd: round(sp.e2b_usd), vision_usd: round(sp.vision_usd), tasks: sp.tasks } : null,
      work: this.work?.summary(a.persona.handle) ?? null,
      latest_reports: this.store.list(a.persona.handle).slice(0, 5).map(ReportStore.card),
    };
  }

  detail(handle: string) {
    const a = this.byHandle.get(handle);
    if (!a) return null;
    return { ...this.agentCard(a), log: [...a.log].reverse().slice(0, 200), thoughts: [...a.thoughts].reverse(), reports: this.store.list(handle).slice(0, 20) };
  }
}

const round = (x: number) => Math.round(x * 1e6) / 1e6;

/** The problems a desktop's last health check shows (empty = healthy). A desktop with no action in
 * `staleMs` (3 task gaps) is flagged once the controller has run that long. */
export function deskIssues(h: DeskHealth | null, lastActionAt: number, staleMs: number, t: number, started: number, memLowMb = 80): string[] {
  const out: string[] = [];
  if (!h) return []; // not checked yet (the first check runs a minute after start)
  if (!h.encoder) out.push("stream encoder down");
  else if (!h.stream_alive) out.push(`stream stale (${h.stream_age_s ?? "no"} s)`);
  if (!h.browser_host) out.push("browser host down");
  if (h.mem_avail_mb !== null && h.mem_avail_mb < memLowMb) out.push(`memory low (${h.mem_avail_mb} MB free)`);
  if (t - started > staleMs && t - lastActionAt > staleMs) out.push(lastActionAt ? `no action for ${Math.round((t - lastActionAt) / 60000)} min` : "no action since the controller started");
  return out;
}
