// The spend ledger: every external dollar the house desktops cost, per UTC day and per agent.
// E2B: seconds each desktop was running (counted by the controller every minute while it runs) x
// E2B's list rate for its size (E2B has no per-sandbox cost call). Vision model: OpenRouter's
// own `usage.cost` of every response. The day cap is hard: when today's spend reaches it, every
// desktop is paused (a paused sandbox bills no compute) until the next UTC day.
import { existsSync, mkdirSync, readFileSync, renameSync, writeFileSync } from "node:fs";
import { dirname } from "node:path";

export interface AgentSpend { e2b_s: number; e2b_usd: number; vision_usd: number; tasks: number; steps: number; in_tokens: number; out_tokens: number }
export interface Day { day: string; e2b_usd: number; vision_usd: number; agents: Record<string, AgentSpend> }

const today = (now: number) => new Date(now).toISOString().slice(0, 10);
const blank = (): AgentSpend => ({ e2b_s: 0, e2b_usd: 0, vision_usd: 0, tasks: 0, steps: 0, in_tokens: 0, out_tokens: 0 });

export class Ledger {
  d: Day;
  history: Day[] = [];
  private file: string;
  private cap: number;
  private now: () => number;
  constructor(file: string, cap: number, now = () => Date.now()) {
    this.file = file; this.cap = cap; this.now = now;
    const saved = existsSync(file) ? JSON.parse(readFileSync(file, "utf8")) : null;
    this.history = saved?.history ?? [];
    this.d = saved?.d ?? { day: today(this.now()), e2b_usd: 0, vision_usd: 0, agents: {} };
    this.roll();
  }
  roll() {
    const t = today(this.now());
    if (this.d.day !== t) {
      this.history = [...this.history, this.d].slice(-14);
      this.d = { day: t, e2b_usd: 0, vision_usd: 0, agents: {} };
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
  vision(h: string, r: { usd: number; steps: number; inTokens: number; outTokens: number }) {
    this.roll();
    const x = this.a(h);
    x.vision_usd += r.usd; x.tasks += 1; x.steps += r.steps; x.in_tokens += r.inTokens; x.out_tokens += r.outTokens;
    this.d.vision_usd += r.usd;
  }
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
