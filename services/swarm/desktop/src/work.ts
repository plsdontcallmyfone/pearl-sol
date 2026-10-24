// The house agents' work loop (owner 2026-10-23: the agents must do real work, not read Cadre pages).
// Every agent has one job (src/roster.ts). Each tick the engine starts the job units that are due on
// the agent's free desktops and keeps the rest running in the background:
//   research  takes its own market hires; the vision model researches in the browser (pages captured
//             as receipts), the brief is checked against those pages and delivered on chain
//   manager   commissions briefs: a current headline from a public news feed or an operator question,
//             hired from the least busy research desk, reviewed by a house source reviewer
//   reviewer  fetches every cited page itself, checks each quote, votes on chain (no model)
//   qa        crawls the live site / API / explorer in the browser (scripted, no model), or checks one
//             main flow with the vision model; files a deduplicated bug list
//   monitor   samples real devnet state every few minutes, opens findings on thresholds, hourly report
//   trader    follows its practice trades (cadre-practice runs them on the strategies contract)
//   digest    every six hours, one report linking everything the others finished
// Outputs are reports (src/reports.ts) and the on-chain market records they name. Nothing goes to the
// town. Everything is house activity: house buyers, house providers, house reviewers, labelled so.
import { existsSync, mkdirSync, readFileSync, renameSync, writeFileSync } from "node:fs";
import { dirname } from "node:path";
import type { Role, StepResult, Visit } from "./desk.ts";
import type { SubmittedReport, TaskResult } from "./vision.ts";
import { applyRules, type Finding, MONITORS, type MonitorCtx, type Point, sampleOf } from "./monitors.ts";
import { lineChart, mdTable, type Report, ReportStore, sha256 } from "./reports.ts";
import { briefMarkdown, checkAgainstVisits, hostOf, NEWS_FEEDS, newsRequest, OPS_QUESTIONS, parseRss, pickStory, quoteMatch, researchTask, verdictOf } from "./research.ts";
import { CADRE, type MarketOps, RESEARCH_OFFER } from "./market.ts";
import { handlesOf, type Job, jobText, type QaSuite } from "./roster.ts";
import type { Wallet } from "../../src/wallet.ts";

/** What the engine needs from the controller (tests pass a fake). */
export interface WorkAgent { handle: string; name: string; wallet: Wallet; roles: Role[]; job: Job }
export interface WorkHost {
  site: string;
  explorer: string;
  /** the public API origin (what a person's browser opens) */
  publicApi: string;
  now(): number;
  event(e: Record<string, unknown>): void;
  /** a line in the agent's work log (its activity console) */
  note(handle: string, kind: string, text: string, extra?: { tx?: string; ref?: string; ok?: boolean }): void;
  /** desktops of this agent that are running and free right now */
  freeRoles(handle: string): Role[];
  /** scripted browser steps on one desktop (visible, no model); null when the desktop is not free */
  script(handle: string, role: Role, kind: string, steps: { url?: string; wait_ms?: number; highlight?: string; scroll?: number }[], o?: { collect?: boolean; viewport?: { width: number; height: number } }): Promise<StepResult[] | null>;
  /** one vision task; `record` keeps every page the browser opens as a receipt */
  vision(handle: string, role: Role, t: { kind: string; text: string; url?: string; report?: boolean; maxSteps?: number; maxUsd?: number; record?: boolean }): Promise<(TaskResult & { visits?: Visit[] }) | null>;
  visionReady(): boolean;
  /** cadre-api on the box (JSON) */
  api(path: string): Promise<any>;
  /** the Cadre node */
  node(method: string, params?: unknown): Promise<any>;
  /** an outside page through the network guard: text (HTML to text) */
  fetchPage(url: string): Promise<{ status: number; url: string; title: string; text: string }>;
  /** an outside document through the network guard: raw body */
  fetchRaw(url: string): Promise<string>;
  solBalances?(addrs: string[]): Promise<(number | null)[]>;
  self(): ReturnType<NonNullable<MonitorCtx["self"]>>;
  /** the practice runner's state for a trader (cadre-practice), or null */
  practice(handle: string): any | null;
  /** tops an agent's CADRE up from the house operator; null when refused */
  fund(handle: string, units: bigint): Promise<string | null>;
  market: MarketOps | null;
}

export interface WorkCfg {
  stateFile: string;
  monitorS: number;
  qaS: number;
  flowS: number;
  newsHireS: number;
  opsHireS: number;
  digestS: number;
  /** a manager's daily CADRE cap (hire value incl. fee and reviewer fee) */
  managerCapCadre: number;
  /** what the house operator may send to the working agents per UTC day, in CADRE */
  fundCapCadre: number;
  judgeFee: bigint;
  /** hire deadline in blocks (at least the offer's turnaround) */
  deadlineBlocks: number;
  /** vision spend cap per research task and per flow check, USD */
  researchMaxUsd: number;
  flowMaxUsd: number;
}

export const workDefaults = (stateFile: string): WorkCfg => ({
  stateFile, monitorS: 300, qaS: 7200, flowS: 10_800, newsHireS: 3600, opsHireS: 7200, digestS: 21_600,
  managerCapCadre: 8, fundCapCadre: 30, judgeFee: CADRE / 50n, deadlineBlocks: 300, researchMaxUsd: 0.04, flowMaxUsd: 0.02,
});

/** CADRE an agent's job needs on hand (storage deposits and fees; a manager also pays its hires), and
 * the SOL a practice trader needs for one inference ticket (about 0.0253 SOL, HANDOFF 47). */
export function needsOf(j: Job): { cadre: number; sol: number } {
  if (j.kind === "manager") return { cadre: 3, sol: 0 };
  if (j.kind === "trader") return { cadre: 0.5, sol: 0.03 };
  if (j.kind === "research" || j.kind === "reviewer") return { cadre: 0.3, sol: 0 };
  return { cadre: 0, sol: 0 };
}
const FLOAT_TARGET: Record<string, number> = { manager: 10, trader: 2, research: 1, reviewer: 1 };

// ------------------------------------------------------------------------ QA suites

export const SITE_PAGES = ["/", "/overview", "/market", "/buy", "/ai", "/agents", "/agent", "/strategies", "/leaderboards", "/discover", "/live", "/mine", "/crank", "/dashboard", "/trade", "/explorer", "/use-cases", "/board"];
export const API_PATHS = ["/v1/ready", "/v1/summary", "/v1/bridge", "/v1/mining", "/v1/crank", "/v1/market", "/v1/market/offers", "/v1/market/hires?limit=20", "/v1/strategies", "/v1/agents", "/v1/inference/models", "/v1/desktops", "/v1/swarms", "/v1/hosted"];
/** Flow checks for the vision model: look, never sign. */
export const FLOWS = [
  { id: "market_hire_review", path: "/market", text: "Open the first service offer listed on this market page. Check that its price, turnaround and a Hire button are shown, click Hire and check that a review screen shows the price and fee before any signature. Do NOT sign or connect a wallet; stop at the review screen." },
  { id: "buy_quote", path: "/buy", text: "On this Buy and bridge page, type 0.01 into the SOL amount field and check that a CADRE quote appears with a price and fee. Do NOT click Buy and do not connect a wallet." },
  { id: "create_agent_form", path: "/agents/new", text: "On this create-an-agent page, check that the form explains what it will create and what it costs, and that leaving the handle empty shows a clear message. Do NOT connect a wallet or sign." },
  { id: "strategies", path: "/strategies", text: "Open the first strategy or trade listed here and check that its entry, exit and post-mortem are shown with links to their chain records. Report anything missing or broken." },
  { id: "live_tile", path: "/live", text: "Check that the live desktops grid shows agents with a current activity, then click one tile and check that its page opens with the desktop picture and a work log." },
  { id: "explorer_search", path: "/explorer", text: "On the explorer, click the newest block, then one transaction in it, and check that the transaction page shows its kind, sender, fee and status." },
];

export interface Issue { id: string; page: string; kind: string; detail: string; first_seen: string; last_seen: string; times: number; open: boolean }

/** Issues a crawl result shows. ERR_ABORTED is a navigation the page cancelled itself (prefetch), not a bug. */
export function issuesOf(r: StepResult, page: string, o: { mobile?: boolean } = {}): { kind: string; detail: string }[] {
  const out: { kind: string; detail: string }[] = [];
  if (r.error) out.push({ kind: "load_failed", detail: r.error });
  else if (r.status !== null && r.status >= 400) out.push({ kind: "http_error", detail: `HTTP ${r.status}` });
  for (const e of r.page_errors) out.push({ kind: "uncaught_error", detail: e.slice(0, 200) });
  for (const e of r.console_errors) out.push({ kind: "console_error", detail: e.slice(0, 200) });
  for (const b of r.bad_responses) out.push({ kind: "request_error", detail: `HTTP ${b.status} ${b.url.replace(/\?.*$/, "").slice(0, 160)}` });
  for (const f of r.failed_requests) if (!/ERR_ABORTED/.test(f.error)) out.push({ kind: "request_failed", detail: `${f.error} ${f.url.replace(/\?.*$/, "").slice(0, 160)}` });
  if (/\bUnavailable\b/.test(r.excerpt)) out.push({ kind: "shows_unavailable", detail: (/[^.]{0,80}\bUnavailable\b[^.]{0,80}/.exec(r.excerpt)?.[0] ?? "Unavailable").trim() });
  if (r.ms !== null && r.ms > 8000) out.push({ kind: "slow", detail: `first paint after ${(r.ms / 1000).toFixed(1)} s` });
  if (o.mobile && r.overflow_px !== null && r.overflow_px > 4) out.push({ kind: "sideways_scroll", detail: `page is ${r.overflow_px} px wider than a 390 px screen` });
  return out.map((x) => ({ ...x, detail: x.detail.replace(/\s+/g, " ") }));
}

const issueId = (page: string, kind: string, detail: string) => sha256(`${page}|${kind}|${detail.replace(/\d+/g, "N")}`).slice(0, 12);

/** Merges one run's issues into the tracked list: new ones open, seen ones refreshed, the rest marked not seen. */
export function mergeIssues(known: Issue[], found: { page: string; kind: string; detail: string }[], pagesRun: Set<string>, at: string): { all: Issue[]; fresh: Issue[] } {
  const by = new Map(known.map((i) => [i.id, i]));
  const seen = new Set<string>();
  const fresh: Issue[] = [];
  for (const f of found) {
    const id = issueId(f.page, f.kind, f.detail);
    if (seen.has(id)) continue;
    seen.add(id);
    const cur = by.get(id);
    if (cur) by.set(id, { ...cur, detail: f.detail, last_seen: at, times: cur.times + 1, open: true });
    else { const i = { id, page: f.page, kind: f.kind, detail: f.detail, first_seen: at, last_seen: at, times: 1, open: true }; by.set(id, i); fresh.push(i); }
  }
  for (const [id, i] of by) if (!seen.has(id) && pagesRun.has(i.page) && i.open) by.set(id, { ...i, open: false });
  const all = [...by.values()].sort((a, b) => Number(b.open) - Number(a.open) || (a.last_seen < b.last_seen ? 1 : -1)).slice(0, 300);
  return { all, fresh };
}

// ------------------------------------------------------------------------ state

interface MonSt { pts: Point[]; open: Finding[]; closed: Finding[]; lastAt: number; reportAt: number; detail?: unknown }
interface AgentSt {
  mon?: MonSt;
  qa?: { issues: Issue[]; lastAt: Record<string, number>; flow: number };
  research?: { attempts: Record<string, number>; offer?: number };
  manager?: { lastAt: number; done: string[]; q: number; day: string; spent: string; hires: number[]; reviewer: number };
  reviewer?: { voted: number[]; attempts: Record<string, number> };
  trader?: { version: number; entry: number | null; explained: number; waitingNoted?: boolean };
  digest?: { lastAt: number };
  fundAt?: number;
}
interface State { agents: Record<string, AgentSt>; fund: { day: string; units: string } }

const day = (t: number) => new Date(t).toISOString().slice(0, 10);
const cad = (u: bigint | string) => (Number(BigInt(u)) / 1e9).toLocaleString("en-US", { maximumFractionDigits: 4 });

export class WorkEngine {
  h: WorkHost;
  cfg: WorkCfg;
  agents: WorkAgent[];
  store: ReportStore;
  st: State;
  running = new Set<string>();
  dirty = false;
  constructor(h: WorkHost, agents: WorkAgent[], store: ReportStore, cfg: WorkCfg) {
    this.h = h; this.agents = agents; this.store = store; this.cfg = cfg;
    let st: State | null = null;
    try { if (existsSync(cfg.stateFile)) st = JSON.parse(readFileSync(cfg.stateFile, "utf8")); } catch { st = null; }
    this.st = st ?? { agents: {}, fund: { day: "", units: "0" } };
    this.st.fund ??= { day: "", units: "0" };
  }
  ag(h: string): AgentSt { return (this.st.agents[h] ??= {}); }
  save() {
    if (!this.dirty) return;
    mkdirSync(dirname(this.cfg.stateFile), { recursive: true });
    writeFileSync(`${this.cfg.stateFile}.tmp`, JSON.stringify(this.st));
    renameSync(`${this.cfg.stateFile}.tmp`, this.cfg.stateFile);
    this.dirty = false;
  }
  by(handle: string) { return this.agents.find((a) => a.handle === handle) ?? null; }

  /** Runs `f` once at a time under `key` (a job unit), in the background. */
  spawn(key: string, f: () => Promise<void>) {
    if (this.running.has(key)) return;
    this.running.add(key);
    void f().catch((e) => this.h.event({ event: "work_error", unit: key, error: String((e as Error)?.message ?? e).slice(0, 240) })).finally(() => { this.running.delete(key); this.dirty = true; });
  }

  report(a: WorkAgent, r: Omit<Report, "id" | "sha256" | "created" | "agent" | "job">): Report {
    const out = this.store.put({ ...r, agent: a.handle, job: jobText(a.handle).title });
    this.h.note(a.handle, "report", `${r.title}`, { ref: out.id, ok: true });
    this.h.event({ event: "work_report", agent: a.handle, kind: r.kind, report: out.id, sha256: out.sha256 });
    return out;
  }

  /** One pass: every agent's due units start in the background. */
  async tick(): Promise<void> {
    const now = this.h.now();
    for (const [i, a] of this.agents.entries()) {
      const j = a.job;
      if (j.kind === "monitor") { const s = this.ag(a.handle).mon; if (!s || now - s.lastAt >= this.cfg.monitorS * 1000 - (i % 10) * 1000) this.spawn(`${a.handle}:monitor`, () => this.monitor(a)); }
      else if (j.kind === "qa") this.qa(a, now);
      else if (j.kind === "research") this.spawn(`${a.handle}:research`, () => this.research(a));
      else if (j.kind === "manager") this.spawn(`${a.handle}:manager`, () => this.manager(a));
      else if (j.kind === "reviewer") this.spawn(`${a.handle}:reviewer`, () => this.reviewer(a));
      else if (j.kind === "trader") this.spawn(`${a.handle}:trader`, () => this.trader(a));
      else if (j.kind === "digest") { const s = this.ag(a.handle).digest; if (!s || now - s.lastAt >= this.cfg.digestS * 1000) this.spawn(`${a.handle}:digest`, () => this.digest(a)); }
      const n = needsOf(j);
      const fa = this.ag(a.handle).fundAt ?? 0;
      if (n.cadre > 0 && now - fa > 1_800_000) this.spawn(`${a.handle}:fund`, () => this.ensureFloat(a));
    }
    this.save();
  }

  /** Keeps a working agent's CADRE at its job's float, from the house operator, inside a daily cap. */
  async ensureFloat(a: WorkAgent): Promise<void> {
    const s = this.ag(a.handle);
    s.fundAt = this.h.now();
    const n = needsOf(a.job);
    const acc = await this.h.node("getAccount", { account: a.wallet.address }).catch(() => null);
    if (!acc) return;
    const bal = BigInt(acc.balance ?? 0);
    if (bal >= BigInt(Math.round(n.cadre * 1e9))) return;
    const want = BigInt(Math.round((FLOAT_TARGET[a.job.kind] ?? n.cadre) * 1e9)) - bal;
    const d = day(this.h.now());
    if (this.st.fund.day !== d) this.st.fund = { day: d, units: "0" };
    const used = BigInt(this.st.fund.units);
    const cap = BigInt(Math.round(this.cfg.fundCapCadre * 1e9));
    if (used + want > cap) { this.h.event({ event: "work_fund_refused", agent: a.handle, want: want.toString(), used: used.toString(), cap: cap.toString() }); return; }
    const tx = await this.h.fund(a.handle, want);
    if (!tx) return;
    this.st.fund.units = (used + want).toString();
    this.dirty = true;
    this.h.note(a.handle, "tx", `received ${cad(want)} CADRE from the house operator for its job (storage deposits and fees)`, { tx, ok: true });
  }

  // ---------------------------------------------------------------- monitor

  ctx(): MonitorCtx {
    return {
      api: (p) => this.h.api(p), node: (m, p) => this.h.node(m, p), solBalances: this.h.solBalances?.bind(this.h), self: () => this.h.self(), now: () => this.h.now(),
      houseAccounts: () => this.agents.map((x) => ({ handle: x.handle, account: x.wallet.address, needs: needsOf(x.job) })),
    };
  }

  async monitor(a: WorkAgent): Promise<void> {
    const t = a.job.target!;
    const def = MONITORS[t];
    const s = (this.ag(a.handle).mon ??= { pts: [], open: [], closed: [], lastAt: 0, reportAt: 0 });
    const now = this.h.now();
    s.lastAt = now;
    let sample;
    try { sample = await sampleOf(t, this.ctx(), s.pts); } catch (e) {
      this.h.note(a.handle, "note", `could not read ${def.sources[0]}: ${String((e as Error).message).slice(0, 120)}`, { ok: false });
      return;
    }
    s.pts.push({ t: now, s: sample.s });
    s.pts = s.pts.filter((p) => now - p.t <= 86_400_000);
    s.detail = sample.detail;
    const r = applyRules(def, s.pts, s.open, now);
    s.open = r.open;
    s.closed = [...r.closed, ...s.closed].filter((f) => now - (f.closed ?? 0) <= 86_400_000).slice(0, 50);
    const key = def.chart.key;
    const v = sample.s[key];
    this.h.note(a.handle, "note", `${def.title}: ${def.chart.label.toLowerCase()} ${v === null || v === undefined ? "unknown" : Number(v).toFixed(v >= 100 ? 0 : 2)} ${def.chart.unit}; ${s.open.length} open findings`);
    for (const f of r.opened) { this.h.note(a.handle, "finding", f.text, { ok: false }); this.h.event({ event: "monitor_finding", agent: a.handle, target: t, rule: f.rule, text: f.text }); }
    for (const f of r.closed) this.h.note(a.handle, "finding", `cleared: ${f.text}`, { ok: true });
    if (r.opened.length) {
      const role = this.h.freeRoles(a.handle)[0];
      if (role) await this.h.script(a.handle, role, "monitor_show", [{ url: def.page(this.h.site), wait_ms: 5000, scroll: 1 }]).catch(() => null);
    }
    if (r.opened.length || r.closed.length || now - s.reportAt >= 3_600_000) {
      s.reportAt = now;
      this.monitorReport(a, s);
    }
    this.dirty = true;
  }

  monitorReport(a: WorkAgent, s: MonSt) {
    const def = MONITORS[a.job.target!];
    const lastS = s.pts[s.pts.length - 1]?.s ?? {};
    const chart = lineChart(s.pts.map((p) => ({ t: p.t, v: p.s[def.chart.key] as number })), { title: def.chart.label, unit: def.chart.unit, threshold: def.chart.threshold });
    const iso = (t: number) => new Date(t).toISOString().replace("T", " ").slice(0, 16) + " UTC";
    const md = [
      `# ${def.title}: ${s.open.length ? `${s.open.length} open finding${s.open.length > 1 ? "s" : ""}` : "no open findings"}`,
      "",
      `Monitor report by @${a.handle}. ${s.pts.length} samples over the last ${((s.pts[s.pts.length - 1]!.t - s.pts[0]!.t) / 3_600_000).toFixed(1)} h, every ${this.cfg.monitorS / 60} min. Sources: ${def.sources.join("; ")}.`,
      "",
      "## Open findings",
      "",
      s.open.length ? mdTable(["since", "rule", "finding"], s.open.map((f) => [iso(f.opened), f.what, f.text])) : "None.",
      "",
      "## Cleared in the last 24 h",
      "",
      s.closed.length ? mdTable(["opened", "cleared", "finding"], s.closed.map((f) => [iso(f.opened), iso(f.closed!), f.text])) : "None.",
      "",
      "## Latest sample",
      "",
      mdTable(["value", "reading"], Object.entries(lastS).map(([k, v]) => [k, v === null ? "unknown" : String(Math.round(Number(v) * 1000) / 1000)])),
      "",
      "## Rules",
      "",
      ...def.rules.map((r) => `- ${r.what}`),
    ].join("\n");
    this.report(a, {
      kind: "monitor", title: `${def.title}: ${s.open.length ? s.open.map((f) => f.what).join("; ") : "all clear"}`.slice(0, 160),
      summary: s.open.length ? s.open.map((f) => f.text).join(". ") : `No threshold crossed; ${def.chart.label.toLowerCase()} ${lastS[def.chart.key] ?? "unknown"} ${def.chart.unit}.`,
      body_md: md, links: [{ label: def.title, url: def.page(this.h.site) }], receipts: { samples: s.pts.length, sources: def.sources, vision_usd: 0 },
      data: { latest: lastS, open: s.open, closed: s.closed, detail: s.detail ?? null, series: s.pts.slice(-288).map((p) => [p.t, p.s[def.chart.key]]) }, chart_svg: chart,
    });
  }

  // ---------------------------------------------------------------- QA

  qa(a: WorkAgent, now: number) {
    const s = (this.ag(a.handle).qa ??= { issues: [], lastAt: {}, flow: 0 });
    const suite = a.job.suite!;
    const every = (suite === "flows" ? this.cfg.flowS : this.cfg.qaS) * 1000;
    for (const [k, role] of a.roles.entries()) {
      const key = `${a.handle}:qa:${role}`;
      if (this.running.has(key) || now - (s.lastAt[role] ?? 0) < every) continue;
      if (!this.h.freeRoles(a.handle).includes(role)) continue;
      if (suite === "flows" && !this.h.visionReady()) continue;
      s.lastAt[role] = now;
      this.dirty = true;
      this.spawn(key, () => this.qaRun(a, suite, role, k));
    }
  }

  async qaPlan(suite: QaSuite, part: number, parts: number): Promise<{ steps: { url: string; wait_ms: number; scroll?: number }[]; mobile?: boolean; label: string }> {
    const split = <T>(xs: T[]) => xs.filter((_, i) => i % parts === part);
    if (suite === "site") return { steps: split(SITE_PAGES).map((p) => ({ url: this.h.site + p, wait_ms: 4000, scroll: 1 })), label: "site pages" };
    if (suite === "mobile") return { steps: SITE_PAGES.map((p) => ({ url: this.h.site + p, wait_ms: 3500 })), mobile: true, label: "site pages at 390 px" };
    // api_explorer: desktop 1 the API, desktop 2 the explorer pages for the newest records
    if (parts > 1 && part === 1) {
      const sum = await this.h.api("/v1/summary").catch(() => null);
      const h = Number(sum?.chain?.height ?? 0);
      const blk = h ? await this.h.node("getBlock", { height: h - 1 }).catch(() => null) : null;
      const tx = blk?.txs?.[0]?.tx_hash ?? blk?.txs?.[0]?.hash ?? null;
      const acct = this.agents[0]?.wallet.address;
      const ex = this.h.explorer;
      return { steps: [{ url: ex, wait_ms: 4000 }, ...(h ? [{ url: `${ex}/block/${h - 1}`, wait_ms: 4000 }] : []), ...(tx ? [{ url: `${ex}/tx/${tx}`, wait_ms: 4000 }] : []), ...(acct ? [{ url: `${ex}/account/${acct}`, wait_ms: 4000 }] : [])], label: "explorer pages" };
    }
    return { steps: API_PATHS.map((p) => ({ url: this.h.publicApi + p, wait_ms: 1500 })), label: "API endpoints" };
  }

  async qaRun(a: WorkAgent, suite: QaSuite, role: Role, part: number): Promise<void> {
    const s = this.ag(a.handle).qa!;
    const at = new Date(this.h.now()).toISOString();
    let found: { page: string; kind: string; detail: string }[] = [];
    let pagesRun = new Set<string>();
    let label = "";
    let receipts: Record<string, unknown> = { vision_usd: 0 };
    let rows: (string | number | null)[][] = [];
    if (suite === "flows") {
      const f = FLOWS[s.flow % FLOWS.length]!;
      s.flow++;
      label = `flow check: ${f.id}`;
      const url = this.h.site + f.path;
      const r = await this.h.vision(a.handle, role, { kind: "qa_flow", url, report: true, maxSteps: 16, maxUsd: this.cfg.flowMaxUsd, text: `QA flow check on the live Cadre site. ${f.text} Then call submit_report: title, a summary of what worked, findings (what you checked, the page URL, a short quote of what the page showed), and problems (each page URL and what is broken or confusing). If everything worked, problems is empty.` });
      if (!r) return;
      pagesRun = new Set([f.path]);
      receipts = { vision_usd: Number(r.usd.toFixed(5)), steps: r.steps, model_finished: r.ok };
      const rep = r.report;
      found = (rep?.problems ?? []).map((p) => ({ page: f.path, kind: "flow_problem", detail: `${f.id}: ${p.what}` }));
      if (!rep) found.push({ page: f.path, kind: "flow_incomplete", detail: `${f.id}: ${r.summary || r.error || "the check did not finish"}` });
      rows = (rep?.findings ?? []).map((x) => [x.claim, x.source_url, x.quote]);
    } else {
      const plan = await this.qaPlan(suite, part, a.roles.length);
      label = plan.label;
      const res = await this.h.script(a.handle, role, `qa_${suite}`, plan.steps, { collect: true, viewport: plan.mobile ? { width: 390, height: 844 } : undefined });
      if (!res) return;
      for (const [i, r] of res.entries()) {
        const page = plan.steps[i]!.url.replace(this.h.site, "").replace(this.h.publicApi, "api:").replace(this.h.explorer, "explorer:") || "/";
        pagesRun.add(page);
        const iss = issuesOf(r, page, { mobile: plan.mobile });
        found.push(...iss.map((x) => ({ page, ...x })));
        rows.push([page, r.status, r.ms, iss.length ? iss.map((x) => x.kind).join(", ") : "ok"]);
      }
      // the API part also checks each endpoint server side: JSON, time
      if (label === "API endpoints") {
        for (const p of API_PATHS) {
          const t0 = this.h.now();
          try { await this.h.api(p); const ms = this.h.now() - t0; if (ms > 3000) found.push({ page: `api:${p}`, kind: "slow", detail: `answered in ${(ms / 1000).toFixed(1)} s on the box` }); }
          catch (e) { found.push({ page: `api:${p}`, kind: "api_error", detail: String((e as Error).message).slice(0, 160) }); }
        }
      }
    }
    const m = mergeIssues(s.issues, found, pagesRun, at);
    s.issues = m.all;
    const open = s.issues.filter((i) => i.open);
    for (const i of m.fresh) this.h.note(a.handle, "finding", `new issue on ${i.page}: ${i.kind} (${i.detail.slice(0, 120)})`, { ok: false });
    const md = [
      `# QA: ${label}, ${m.fresh.length} new, ${open.length} open`,
      "",
      `QA run by @${a.handle} on ${at.replace("T", " ").slice(0, 16)} UTC. ${suite === "flows" ? "Checked by the vision model in a real browser; nothing was signed." : `${pagesRun.size} pages loaded in a real browser on the agent's desktop, console, page errors and every request recorded.`}`,
      "",
      "## New in this run",
      "",
      m.fresh.length ? mdTable(["page", "issue", "detail"], m.fresh.map((i) => [i.page, i.kind, i.detail])) : "Nothing new.",
      "",
      "## Open issues (all runs)",
      "",
      open.length ? mdTable(["page", "issue", "detail", "first seen", "times"], open.slice(0, 60).map((i) => [i.page, i.kind, i.detail, i.first_seen.slice(0, 16).replace("T", " "), i.times])) : "None open.",
      "",
      "## This run",
      "",
      suite === "flows" ? (rows.length ? mdTable(["checked", "page", "seen"], rows) : "No findings reported.") : mdTable(["page", "HTTP", "ms", "result"], rows),
    ].join("\n");
    this.report(a, { kind: "qa", title: `QA ${label}: ${m.fresh.length} new, ${open.length} open issues`, summary: m.fresh.length ? m.fresh.slice(0, 3).map((i) => `${i.page}: ${i.kind}`).join("; ") : `No new issues; ${open.length} still open.`, body_md: md, links: [{ label: "site", url: this.h.site }], receipts, data: { fresh: m.fresh, open, run: rows } });
    this.dirty = true;
  }

  // ---------------------------------------------------------------- research (provider)

  async research(a: WorkAgent): Promise<void> {
    const mk = this.h.market;
    if (!mk) return;
    const s = (this.ag(a.handle).research ??= { attempts: {} });
    if (s.offer === undefined) {
      const o = await mk.ensureOffer(a.wallet, a.handle, RESEARCH_OFFER);
      if (!o) return;
      s.offer = o.id;
      this.dirty = true;
      this.h.note(a.handle, "note", `offer #${o.id} "${o.title}" is listed on the market at ${cad(o.price)} CADRE`);
    }
    await mk.refresh();
    const mine = mk.hires().filter((x) => x.provider === a.wallet.address && x.status === "open" && x.deadline - mk.height > 10 && (s.attempts[x.id] ?? 0) < 2);
    for (const hire of mine) {
      const role = this.h.freeRoles(a.handle).find((r) => !this.running.has(`${a.handle}:brief:${r}`));
      if (!role || !this.h.visionReady()) break;
      if ([...this.running].some((k) => k === `${a.handle}:hire:${hire.id}`)) continue;
      this.running.add(`${a.handle}:hire:${hire.id}`);
      this.spawn(`${a.handle}:brief:${role}`, () => this.brief(a, role, hire.id).finally(() => this.running.delete(`${a.handle}:hire:${hire.id}`)));
    }
  }

  async brief(a: WorkAgent, role: Role, hireId: number): Promise<void> {
    const mk = this.h.market!;
    const s = this.ag(a.handle).research!;
    const hire = mk.hires().find((x) => x.id === hireId)!;
    const req = await mk.getBlob(hire.request_uri);
    const request = String(req?.request ?? "").slice(0, 1200);
    const links = (Array.isArray(req?.links) ? req.links : []).filter((x: unknown) => typeof x === "string" && /^https:\/\//.test(x as string)).slice(0, 2) as string[];
    if (!request) { s.attempts[hireId] = (s.attempts[hireId] ?? 0) + 1; this.h.note(a.handle, "note", `hire #${hireId}: request blob not available yet`, { ok: false }); return; }
    this.h.note(a.handle, "note", `working on hire #${hireId}: ${request.slice(0, 140)}`);
    const t = researchTask(request, links);
    const r = await this.h.vision(a.handle, role, { kind: "research", text: t.text, url: t.url, report: true, maxSteps: 30, maxUsd: this.cfg.researchMaxUsd, record: true });
    if (!r) return; // budget, cap or vision paused: the hire waits, no attempt used
    s.attempts[hireId] = (s.attempts[hireId] ?? 0) + 1;
    this.dirty = true;
    const visits = r.visits ?? [];
    if (!r.report) {
      this.h.note(a.handle, "note", `hire #${hireId}: no brief handed in (${r.summary || r.error || "stopped"}), attempt ${s.attempts[hireId]} of 2`, { ok: false });
      if (s.attempts[hireId]! >= 2) await this.declineHire(a, hireId, "two research attempts handed in no brief");
      return;
    }
    const c = checkAgainstVisits(r.report, visits);
    if (!c.ok) {
      this.h.note(a.handle, "note", `hire #${hireId}: brief not delivered: ${c.why}`, { ok: false });
      if (s.attempts[hireId]! >= 2) await this.declineHire(a, hireId, `brief failed its own source check twice: ${c.why}`);
      return;
    }
    const md = briefMarkdown({ hire: hireId, request, r: r.report, c, agent: a.handle, job: jobText(a.handle).title });
    const pages = visits.filter((v) => /^https?:/.test(v.url)).map((v) => ({ url: v.url, title: v.title, at: new Date(v.t).toISOString(), text_sha256: sha256(v.text), text_chars: v.text.length }));
    const d = await mk.deliver(a.wallet, a.handle, hireId, {
      title: r.report.title, summary: r.report.summary, text: md, text_sha256: sha256(md), request, findings: c.findings, check: { verified: c.verified, hosts: c.hosts, why: c.why },
      pages_opened: pages, written_by: { operator: "vision model on the agent's desktop", model: "see the controller's vision_model", usd: Number(r.usd.toFixed(5)), steps: r.steps, note: "not a paid Cadre thought: the brief is written by the desktop operator model from the pages it opened" },
    });
    this.h.note(a.handle, "tx", `delivered hire #${hireId}: "${r.report.title}"`, { tx: d.tx, ok: true });
    this.report(a, { kind: "research", title: r.report.title, summary: r.report.summary.slice(0, 600), body_md: md, links: [{ label: `market hire #${hireId}`, url: `${this.h.site}/market/hires/${hireId}` }, ...c.findings.map((f) => ({ label: hostOf(f.source_url), url: f.source_url }))], receipts: { hire: hireId, deliver_tx: d.tx, delivery_blob: d.uri, vision_usd: Number(r.usd.toFixed(5)), steps: r.steps, pages_opened: pages }, data: { request, findings: c.findings, check: c } });
  }

  async declineHire(a: WorkAgent, hireId: number, reason: string) {
    try {
      const r = await this.h.market!.decline(a.wallet, a.handle, hireId, reason);
      this.h.note(a.handle, "tx", `declined hire #${hireId} (everything refunded to the buyer): ${reason}`, { tx: r.tx, ok: true });
    } catch (e) { this.h.note(a.handle, "note", `decline of hire #${hireId} failed: ${String((e as Error).message).slice(0, 120)}`, { ok: false }); }
  }

  // ---------------------------------------------------------------- manager (buyer)

  async manager(a: WorkAgent): Promise<void> {
    const mk = this.h.market;
    if (!mk) return;
    const s = (this.ag(a.handle).manager ??= { lastAt: 0, done: [], q: 0, day: "", spent: "0", hires: [], reviewer: 0 });
    const now = this.h.now();
    await mk.refresh();
    // refunds: my hires past their deadline
    for (const h of mk.hires().filter((x) => x.buyer === a.wallet.address && (x.status === "open" || x.status === "delivered") && mk.height > x.deadline)) {
      try { const r = await mk.expire(a.wallet, a.handle, h.id); this.h.note(a.handle, "tx", `hire #${h.id} expired: refunded`, { tx: r.tx, ok: true }); } catch { /* next pass */ }
    }
    const every = (a.job.desk === "news" ? this.cfg.newsHireS : this.cfg.opsHireS) * 1000;
    if (now - s.lastAt < every) return;
    if (s.day !== day(now)) { s.day = day(now); s.spent = "0"; }
    const providers = new Set(handlesOf("research").map((h) => this.by(h)?.wallet.address).filter(Boolean) as string[]);
    const reviewers = handlesOf("reviewer").map((h) => this.by(h)).filter(Boolean) as WorkAgent[];
    if (!providers.size || !reviewers.length) return;
    const load = (addr: string) => mk.hires().filter((x) => x.provider === addr && x.status === "open").length;
    const slots = (addr: string) => this.agents.find((x) => x.wallet.address === addr)?.roles.length ?? 1;
    const offers = mk.offers().filter((o) => o.active && o.title === RESEARCH_OFFER.title && providers.has(o.provider) && load(o.provider) < slots(o.provider));
    if (!offers.length) { s.lastAt = now - every / 2; return; } // every desk busy: look again in half an interval
    offers.sort((x, y) => load(x.provider) / slots(x.provider) - load(y.provider) / slots(y.provider) || x.hires - y.hires);
    const offer = offers[0]!;
    let request: string, links: string[] = [], source: Record<string, unknown>;
    if (a.job.desk === "news") {
      const items = (await Promise.all(NEWS_FEEDS.map(async (f) => parseRss(await this.h.fetchRaw(f.url).catch(() => ""), f.name)))).flat();
      const story = pickStory(items, new Set(s.done), now);
      if (!story) { this.h.note(a.handle, "note", `no new story in ${NEWS_FEEDS.length} feeds (${items.length} items read)`); s.lastAt = now; return; }
      request = newsRequest(story); links = [story.link]; source = { kind: "news", feed: story.feed, headline: story.title, link: story.link, published: story.published ? new Date(story.published).toISOString() : null };
      s.done = [...s.done, story.link].slice(-300);
      const role = this.h.freeRoles(a.handle)[0];
      if (role) void this.h.script(a.handle, role, "read_story", [{ url: story.link, wait_ms: 5000, scroll: 2 }]).catch(() => null);
    } else {
      request = OPS_QUESTIONS[s.q % OPS_QUESTIONS.length]!; s.q++; source = { kind: "operator question", bank_index: (s.q - 1) % OPS_QUESTIONS.length };
    }
    const q = await mk.ix.quote(offer.id, this.cfg.judgeFee);
    const total = BigInt(q.total);
    const cap = BigInt(Math.round(this.cfg.managerCapCadre * 1e9));
    if (BigInt(s.spent) + total > cap) { this.h.note(a.handle, "note", `daily hiring cap reached (${cad(s.spent)} of ${this.cfg.managerCapCadre} CADRE)`); s.lastAt = now; return; }
    const reviewer = reviewers[s.reviewer % reviewers.length]!;
    s.reviewer++;
    const provider = this.agents.find((x) => x.wallet.address === offer.provider);
    const r = await mk.hire(a.wallet, a.handle, offer, { request, links, source, kind: "research", reviewer: reviewer.wallet.address }, reviewer.wallet.address, this.cfg.judgeFee, Math.max(offer.turnaround, this.cfg.deadlineBlocks));
    s.lastAt = now;
    s.spent = (BigInt(s.spent) + r.total).toString();
    if (r.id !== null) s.hires = [...s.hires, r.id].slice(-200);
    this.dirty = true;
    this.h.note(a.handle, "tx", `hired @${provider?.handle ?? offer.provider.slice(0, 8)} (hire #${r.id}) for ${cad(r.total)} CADRE, reviewer @${reviewer.handle}: ${request.slice(0, 120)}`, { tx: r.tx, ok: true });
    const role = this.h.freeRoles(a.handle)[0];
    if (role && r.id !== null) void this.h.script(a.handle, role, "hire_page", [{ url: `${this.h.site}/market/hires/${r.id}`, wait_ms: 5000 }]).catch(() => null);
    this.report(a, {
      kind: "commission", title: `Commissioned hire #${r.id}: ${request.slice(0, 100)}`, summary: `Hired @${provider?.handle ?? "?"} for ${cad(r.total)} CADRE (price ${cad(q.price)}, market fee, reviewer fee ${cad(this.cfg.judgeFee)}), reviewed by @${reviewer.handle}. House hire.`,
      body_md: [`# Commission: market hire #${r.id}`, "", `**Question:** ${request}`, "", ...(links.length ? [`**Starting link:** ${links[0]}`, ""] : []), mdTable(["", ""], [["provider", `@${provider?.handle ?? offer.provider}`], ["reviewer (named evaluator)", `@${reviewer.handle}`], ["paid into escrow", `${cad(r.total)} CADRE`], ["deadline", `${Math.max(offer.turnaround, this.cfg.deadlineBlocks)} blocks`], ["tx", r.tx]]), "", "House activity: buyer, provider and reviewer are house agents; cadre-api reports it as house volume, never organic."].join("\n"),
      links: [{ label: `market hire #${r.id}`, url: `${this.h.site}/market/hires/${r.id}` }, ...links.map((l) => ({ label: hostOf(l), url: l }))], receipts: { hire_tx: r.tx, total_units: r.total.toString() }, data: { request, source, offer: offer.id },
    });
  }

  // ---------------------------------------------------------------- reviewer (evaluator)

  async reviewer(a: WorkAgent): Promise<void> {
    const mk = this.h.market;
    if (!mk) return;
    const s = (this.ag(a.handle).reviewer ??= { voted: [], attempts: {} });
    await mk.refresh();
    const due = mk.hires().filter((x) => x.status === "delivered" && x.evaluators.includes(a.wallet.address) && !x.voted.includes(a.wallet.address) && !s.voted.includes(x.id) && mk.height <= x.deadline && (s.attempts[x.id] ?? 0) < 3);
    for (const h of due.slice(0, 1)) {
      s.attempts[h.id] = (s.attempts[h.id] ?? 0) + 1;
      this.dirty = true;
      const doc = h.result_uri ? await mk.getBlob(h.result_uri) : null;
      if (!doc) { this.h.note(a.handle, "note", `hire #${h.id}: delivery blob not available yet`, { ok: false }); continue; }
      const findings: { claim: string; source_url: string; quote: string }[] = (Array.isArray(doc.findings) ? doc.findings : []).slice(0, 8).map((f: any) => ({ claim: String(f?.claim ?? ""), source_url: String(f?.source_url ?? ""), quote: String(f?.quote ?? "") }));
      this.h.note(a.handle, "note", `reviewing hire #${h.id}: checking ${findings.length} quotes against their pages`);
      const role = this.h.freeRoles(a.handle)[0];
      const show = role ? this.h.script(a.handle, role, "check_sources", findings.filter((f) => /^https:\/\//.test(f.source_url)).slice(0, 4).map((f) => ({ url: f.source_url, wait_ms: 3500, highlight: f.quote }))).catch(() => null) : Promise.resolve(null);
      const rows = await Promise.all(findings.map(async (f) => {
        if (!/^https:\/\//.test(f.source_url)) return { ...f, host: "", status: null as number | null, match: "none" as const, score: 0, why: "not an https URL" };
        try {
          const p = await this.h.fetchPage(f.source_url);
          const m = p.status < 400 ? quoteMatch(`${p.title}\n${p.text}`, f.quote) : { match: "none" as const, score: 0 };
          return { ...f, host: hostOf(f.source_url), status: p.status, match: m.match, score: m.score, why: p.status >= 400 ? `HTTP ${p.status}` : m.match === "none" ? "quote not on the page" : "" };
        } catch (e) { return { ...f, host: hostOf(f.source_url), status: null, match: "none" as const, score: 0, why: String((e as Error).message).slice(0, 80) }; }
      }));
      const shown = await show;
      // a page our server cannot fetch (bot wall) but the reviewer's own browser showed with the quote marked counts too
      for (const [i, sr] of (shown ?? []).entries()) {
        const row = rows.find((r) => r.source_url === findings.filter((f) => /^https:\/\//.test(f.source_url))[i]?.source_url);
        if (row && row.match === "none" && sr.found) { (row as any).match = "close"; row.why = "found in the reviewer's browser (the server fetch did not show it)"; }
      }
      const v = verdictOf(rows);
      const tableMd = mdTable(["claim", "source", "HTTP", "quote on page", "note"], rows.map((r) => [r.claim.slice(0, 80), r.source_url, r.status, r.match, r.why]));
      const vr = await mk.verdict(a.wallet, a.handle, h.id, v.accept, { reason: v.reason, rows, rubric: "accept when at least 2 quotes are found on at least 2 distinct sites and at least half of all quotes are found" });
      s.voted = [...s.voted, h.id].slice(-500);
      this.h.note(a.handle, "tx", `${v.accept ? "ACCEPTED" : "REJECTED"} hire #${h.id}: ${v.reason}`, { tx: vr.tx, ok: true });
      this.report(a, { kind: "verdict", title: `${v.accept ? "Accepted" : "Rejected"} hire #${h.id}: ${String(doc.title ?? "").slice(0, 100)}`, summary: v.reason, body_md: [`# Review of market hire #${h.id}: ${v.accept ? "ACCEPT" : "REJECT"}`, "", `Reviewer @${a.handle} fetched every cited page from the box and checked each quote (no language model). ${v.reason}.`, "", tableMd].join("\n"), links: [{ label: `market hire #${h.id}`, url: `${this.h.site}/market/hires/${h.id}` }], receipts: { verdict_tx: vr.tx, verdict_blob: vr.uri, vision_usd: 0 }, data: { rows, accept: v.accept } });
    }
  }

  // ---------------------------------------------------------------- trader (practice)

  async trader(a: WorkAgent): Promise<void> {
    const s = (this.ag(a.handle).trader ??= { version: 0, entry: null, explained: 0 });
    const p = this.h.practice(a.handle);
    if (!p) {
      if (!s.waitingNoted) { this.h.note(a.handle, "note", "waiting for cadre-practice to run this trader (STRAT_DESK_TRADERS, docs/HOUSE-AGENTS.md)"); s.waitingNoted = true; this.dirty = true; }
      return;
    }
    s.waitingNoted = false;
    const role = this.h.freeRoles(a.handle)[0];
    if (p.version && p.version !== s.version) {
      s.version = p.version; this.dirty = true;
      this.h.note(a.handle, "note", `strategy version ${p.version} committed on the strategies contract`, { ok: true });
      if (role) void this.h.script(a.handle, role, "strategy_page", [{ url: `${this.h.site}/strategies/versions/${p.version}`, wait_ms: 5000 }]).catch(() => null);
    }
    const open = p.open?.entry ?? null;
    if (open !== s.entry) {
      if (open) {
        this.h.note(a.handle, "trade", `practice entry #${open} (${p.open.stage}) in pool ${String(p.open.pool).slice(0, 8)}..., size ${(Number(p.open.size ?? 0) / 1e9).toFixed(4)} pretend SOL`, { tx: p.open.entry_tx, ok: true });
        if (role) void this.h.script(a.handle, role, "trade_page", [{ url: `${this.h.site}/strategies/trades/${open}`, wait_ms: 5000 }]).catch(() => null);
      }
      s.entry = open; this.dirty = true;
    }
    const hist: any[] = Array.isArray(p.history) ? p.history : [];
    for (const t of hist.slice(s.explained)) {
      const pnl = Number(t.pnl ?? 0) / 1e9;
      this.h.note(a.handle, "trade", `practice trade #${t.entry} closed: ${t.outcome}, ${pnl >= 0 ? "+" : ""}${pnl.toFixed(5)} pretend SOL, post-mortem committed`, { ok: true });
      const eq = hist.slice(0, hist.indexOf(t) + 1).reduce((x, y) => x + Number(y.pnl ?? 0), 0) / 1e9;
      this.report(a, { kind: "trade", title: `Practice trade #${t.entry}: ${t.outcome}, ${pnl >= 0 ? "+" : ""}${pnl.toFixed(5)} SOL`, summary: `Version ${t.version}, outcome ${t.outcome}, P&L ${pnl.toFixed(5)} pretend SOL; running total ${eq.toFixed(5)} pretend SOL over ${hist.indexOf(t) + 1} explained trades. Practice mode: real mainnet prices, pretend balance.`, body_md: [`# Practice trade #${t.entry}`, "", mdTable(["", ""], [["strategy version", t.version], ["outcome", t.outcome], ["P&L (pretend SOL)", pnl.toFixed(6)], ["post-mortem buckets (bits)", t.buckets], ["running P&L", eq.toFixed(6)]]), "", "Every step (version, entry, exit, post-mortem) is committed on the Cadre strategies contract; the trade page links each record."].join("\n"), links: [{ label: `trade #${t.entry}`, url: `${this.h.site}/strategies/trades/${t.entry}` }], receipts: { strategies_contract: "deploy/strategies-devnet.json", vision_usd: 0 }, data: { trade: t } });
    }
    if (hist.length !== s.explained) { s.explained = hist.length; this.dirty = true; }
  }

  // ---------------------------------------------------------------- digest

  async digest(a: WorkAgent): Promise<void> {
    const s = (this.ag(a.handle).digest ??= { lastAt: 0 });
    const now = this.h.now();
    const since = s.lastAt || now - this.cfg.digestS * 1000;
    s.lastAt = now;
    this.dirty = true;
    const all = this.store.since(this.agents.map((x) => x.handle).filter((h) => h !== a.handle), since);
    const of = (k: string) => all.filter((r) => r.kind === k);
    const line = (r: Report) => `- @${r.agent}: ${r.title}${r.links[0] ? ` (${r.links[0].url})` : ""}`;
    const latestMon = new Map<string, Report>();
    for (const r of of("monitor")) if (!latestMon.has(r.agent)) latestMon.set(r.agent, r);
    const latestQa = new Map<string, Report>();
    for (const r of of("qa")) if (!latestQa.has(r.agent)) latestQa.set(r.agent, r);
    const md = [
      `# House work digest, ${new Date(since).toISOString().slice(0, 16).replace("T", " ")} to ${new Date(now).toISOString().slice(11, 16)} UTC`,
      "",
      `${all.length} reports from ${new Set(all.map((r) => r.agent)).size} house agents. Every item links to its record.`,
      "",
      `## Research briefs delivered (${of("research").length})`, "", ...(of("research").map(line).length ? of("research").map(line) : ["None."]), "",
      `## Verdicts (${of("verdict").length})`, "", ...(of("verdict").map(line).length ? of("verdict").map(line) : ["None."]), "",
      `## Commissions (${of("commission").length})`, "", ...(of("commission").map(line).length ? of("commission").map(line) : ["None."]), "",
      "## Monitors (latest each)", "", ...([...latestMon.values()].map((r) => `- @${r.agent}: ${r.title}`).length ? [...latestMon.values()].map((r) => `- @${r.agent}: ${r.title}`) : ["No monitor reports."]), "",
      "## QA (latest each)", "", ...([...latestQa.values()].map((r) => `- @${r.agent}: ${r.title}`).length ? [...latestQa.values()].map((r) => `- @${r.agent}: ${r.title}`) : ["No QA runs."]), "",
      `## Practice trades closed (${of("trade").length})`, "", ...(of("trade").map(line).length ? of("trade").map(line) : ["None."]),
    ].join("\n");
    const role = this.h.freeRoles(a.handle)[0];
    if (role) void this.h.script(a.handle, role, "digest_show", [{ url: `${this.h.site}/market/mine`, wait_ms: 3000 }, { url: `${this.h.site}/live`, wait_ms: 4000 }]).catch(() => null);
    this.report(a, { kind: "digest", title: `Work digest: ${of("research").length} briefs, ${of("verdict").length} verdicts, ${[...latestMon.values()].filter((r) => !r.title.endsWith("all clear")).length} monitors with findings`, summary: `${all.length} reports in the window.`, body_md: md, links: [], receipts: { reports: all.map((r) => ({ id: r.id, sha256: r.sha256 })) }, data: { counts: Object.fromEntries(["research", "verdict", "commission", "monitor", "qa", "trade"].map((k) => [k, of(k).length])) } });
  }

  /** Per-agent work summary for the status card. */
  summary(handle: string): Record<string, unknown> {
    const s = this.st.agents[handle] ?? {};
    const a = this.by(handle);
    const out: Record<string, unknown> = { reports: this.store.list(handle).length };
    if (s.mon) out.monitor = { samples: s.mon.pts.length, open_findings: s.mon.open, latest: s.mon.pts[s.mon.pts.length - 1]?.s ?? null };
    if (s.qa) out.qa = { open_issues: s.qa.issues.filter((i) => i.open).length, last_runs: s.qa.lastAt };
    if (s.manager) out.manager = { hires: s.manager.hires.slice(-10), spent_today_cadre: cad(s.manager.spent), cap_cadre: this.cfg.managerCapCadre };
    if (s.reviewer) out.reviewer = { voted: s.reviewer.voted.slice(-10) };
    if (s.research && this.h.market && a) out.research = { offer: s.research.offer ?? null, open_hires: this.h.market.hires().filter((x) => x.provider === a.wallet.address && x.status === "open").map((x) => x.id) };
    if (s.trader) out.trader = s.trader;
    return out;
  }
}
