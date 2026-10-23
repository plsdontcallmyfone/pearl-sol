// The spend ledger: every external dollar the house desktops cost, per UTC day and per agent.
// E2B: seconds each desktop was running (counted by the controller every minute while it runs) x
// E2B's list rate for its size (E2B has no per-sandbox cost call). Vision model: OpenRouter's
// own `usage.cost` of every response. The day cap is hard: when today's spend reaches it, every
// desktop is paused (a paused sandbox bills no compute) until the next UTC day.
import { existsSync, mkdirSync, readFileSync, renameSync, writeFileSync } from "node:fs";
import { dirname } from "node:path";

export interface AgentSpend { e2b_s: number; e2b_usd: number; vision_usd: number; tasks: number; steps: number; in_tokens: number; out_tokens: number }
export interface Day {
  day: string; e2b_usd: number;
  /** every vision dollar of the day: tasks plus credit probes */
  vision_usd: number;
  /** the credit probes' share of vision_usd (src/credits.ts); absent in files written before 2026-10-23 */
  probe_usd?: number;
  /** vision tasks booked and how many of them failed for lack of model credit */
  vision_tasks?: number;
  credit_failures?: number;
  /** the cap in force that day (DESK_USD_PER_DAY) */
  cap_usd?: number;
  agents: Record<string, AgentSpend>;
}

/** One finished (or running) day as the spend log and /status show it. */
export function daySummary(d: Day, cap: number) {
  const r = (x: number) => Math.round(x * 1e6) / 1e6;
  const total = d.e2b_usd + d.vision_usd;
  const c = d.cap_usd ?? cap;
  return {
    day: d.day, e2b_usd: r(d.e2b_usd), vision_usd: r(d.vision_usd), probe_usd: r(d.probe_usd ?? 0), total_usd: r(total),
    cap_usd: c, within_cap: total <= c + 1e-9, vision_share: total > 0 ? r(d.vision_usd / total) : null,
    vision_tasks: d.vision_tasks ?? Object.values(d.agents).reduce((s, x) => s + x.tasks, 0), credit_failures: d.credit_failures ?? 0,
  };
}

const today = (now: number) => new Date(now).toISOString().slice(0, 10);
const blank = (): AgentSpend => ({ e2b_s: 0, e2b_usd: 0, vision_usd: 0, tasks: 0, steps: 0, in_tokens: 0, out_tokens: 0 });

export class Ledger {
  d: Day;
  history: Day[] = [];
  private file: string;
  private cap: number;
  private now: () => number;
  /** called once per finished UTC day, with the day that just ended (the controller logs spend_day) */
  onDayEnd: ((d: Day) => void) | null = null;
  constructor(file: string, cap: number, now = () => Date.now(), onDayEnd?: (d: Day) => void) {
    this.file = file; this.cap = cap; this.now = now; this.onDayEnd = onDayEnd ?? null;
    const saved = existsSync(file) ? JSON.parse(readFileSync(file, "utf8")) : null;
    this.history = saved?.history ?? [];
    this.d = saved?.d ?? { day: today(this.now()), e2b_usd: 0, vision_usd: 0, agents: {} };
    this.roll();
  }
  roll() {
    const t = today(this.now());
    if (this.d.day !== t) {
      const ended = this.d;
      ended.cap_usd ??= this.cap;
      this.history = [...this.history, ended].slice(-14);
      this.d = { day: t, e2b_usd: 0, vision_usd: 0, probe_usd: 0, vision_tasks: 0, credit_failures: 0, cap_usd: this.cap, agents: {} };
      try { this.onDayEnd?.(ended); } catch { /* logging only */ }
    }
  }
  a(h: string) { return (this.d.agents[h] ??= blank()); }
  e2b(h: string, seconds: number, usdPerS: number) {
    this.roll();
    const x = this.a(h);
    x.e2b_s += seconds;
    x.e2b_usd += seconds * usdPerS;
    this.d.e2b_usd += seconds * usdPerS;
  }
  vision(h: string, r: { usd: number; steps: number; inTokens: number; outTokens: number; credit?: unknown }) {
    this.roll();
    const x = this.a(h);
    x.vision_usd += r.usd; x.tasks += 1; x.steps += r.steps; x.in_tokens += r.inTokens; x.out_tokens += r.outTokens;
    this.d.vision_usd += r.usd;
    this.d.vision_tasks = (this.d.vision_tasks ?? 0) + 1;
    if (r.credit) this.d.credit_failures = (this.d.credit_failures ?? 0) + 1;
  }
  /** a credit probe's cost (OpenRouter usage.cost): vision spend, not tied to an agent */
  probe(usd: number) {
    this.roll();
    this.d.vision_usd += usd;
    this.d.probe_usd = (this.d.probe_usd ?? 0) + usd;
  }
  /** today plus the kept history, newest first, each with within_cap */
  days() { this.roll(); return [this.d, ...[...this.history].reverse()].map((d) => daySummary(d, this.cap)); }
  get spent() { return this.d.e2b_usd + this.d.vision_usd; }
  get capped() { this.roll(); return this.spent >= this.cap; }
  /** seconds left in the UTC day */
  secondsLeft() { const n = this.now(); return Math.max(0, (Date.UTC(new Date(n).getUTCFullYear(), new Date(n).getUTCMonth(), new Date(n).getUTCDate() + 1) - n) / 1000); }
  /** what vision may still spend today if the running desktops keep running to midnight */
  visionHeadroom(runningUsdPerS: number) { return this.cap - this.spent - runningUsdPerS * this.secondsLeft(); }
  save() {
    mkdirSync(dirname(this.file), { recursive: true });
    writeFileSync(`${this.file}.tmp`, JSON.stringify({ d: this.d, history: this.history }));
    renameSync(`${this.file}.tmp`, this.file);
  }
  /** Measured USD per agent hour today: (E2B + vision) / agent hours, over agents that ran. */
  measured() {
    const rows = Object.entries(this.d.agents).filter(([, x]) => x.e2b_s > 0);
    const hours = rows.reduce((s, [, x]) => s + x.e2b_s, 0) / 3600; // desktop hours
    const usd = rows.reduce((s, [, x]) => s + x.e2b_usd + x.vision_usd, 0);
    const vis = rows.reduce((s, [, x]) => s + x.vision_usd, 0);
    return { agents: rows.length, desktop_hours: hours, usd, vision_usd: vis, usd_per_desktop_hour: hours > 0 ? usd / hours : null };
  }
}
