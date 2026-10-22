// The toolbelt meter (docs/TOOLBELT.md 4, 7): one per agent, so a swarm of N agents has N meters,
// each with its own prepaid balance, caps, rates and receipt sequence. It is the prepaid-balance
// model of the inference gateway applied to tools: the agent tops up once from its own SOL (one
// Solana transfer from its own wallet, the same cadre-api builder the runtime already uses for the
// agent's Solana actions, inside the spec's daily SOL cap), then each tool call draws on the
// balance with no transaction on the call path, and every call leaves a receipt.
//
// Admission is by an upper bound (reserve), the booking is the real cost (commit). A call is refused
// before it runs when it would pass: the balance, the capability's lamports_per_day, its
// calls_per_day, its per_minute rate, a capability's own count (sends, PRs, commits, minutes, KB), or
// the runtime-wide USD cap that protects the operator's provider bills across all agents.
import { existsSync, mkdirSync, readFileSync, renameSync, writeFileSync } from "node:fs";
import { dirname } from "node:path";
import { dayOf } from "../../../src/budget.ts";
import type { Capability, ToolbeltSpec } from "./spec.ts";

export class ToolRefused extends Error {
  code: string;
  constructor(code: string, message: string) { super(message); this.code = code; }
}

export interface CapDay { lamports: string; calls: number; units: Record<string, number> }
export interface Topup { signature: string; lamports: string; t_ms: number; verified: boolean }
export interface MeterState {
  day: string;
  balance: string; // lamports credited by verified top-ups minus lamports drawn
  credited: string;
  drawn: string;
  reserved: string;
  topups: Topup[];
  caps: Record<string, CapDay>;
  minute: Record<string, number[]>; // call start times in the last 60 s
  seq: number; // next receipt seq
}

const fresh = (now: number): MeterState => ({ day: dayOf(now), balance: "0", credited: "0", drawn: "0", reserved: "0", topups: [], caps: {}, minute: {}, seq: 0 });

/** Process-wide spend across every agent of this runtime (USD of outside cost), per UTC day. */
export class RuntimeLedger {
  day = "";
  usd = 0;
  reserved = 0;
  capUsd: number;
  file: string | null;
  constructor(capUsd: number, file: string | null = null, now = Date.now()) {
    this.capUsd = capUsd;
    this.file = file;
    if (file && existsSync(file)) { const j = JSON.parse(readFileSync(file, "utf8")); this.day = j.day; this.usd = j.usd; }
    this.roll(now);
  }
  roll(now: number) { if (this.day !== dayOf(now)) { this.day = dayOf(now); this.usd = 0; this.reserved = 0; } }
  admits(usd: number, now: number) { this.roll(now); return this.usd + this.reserved + usd <= this.capUsd; }
  save() { if (!this.file) return; mkdirSync(dirname(this.file), { recursive: true }); writeFileSync(`${this.file}.tmp`, JSON.stringify({ day: this.day, usd: this.usd })); renameSync(`${this.file}.tmp`, this.file); }
}

export interface Reservation { cap: Capability; lamports: bigint; usd: number; units: Record<string, number>; t: number }

/** Limits a capability may carry besides lamports and calls (per UTC day), by unit name. */
export function unitCaps(t: ToolbeltSpec, cap: Capability): Record<string, number> {
  switch (cap) {
    case "computer": return t.computer ? { seconds: t.computer.minutes_per_day * 60 } : {};
    case "files": return t.files ? { kb: t.files.max_kb_per_day } : {};
    case "email": return t.email ? { sends: t.email.sends_per_day } : {};
    case "code": return t.code?.github ? { prs: t.code.github.prs_per_day, commits: t.code.github.commits_per_day } : {};
    default: return {};
  }
}

export class Meter {
  st: MeterState;
  spec: ToolbeltSpec;
  file: string;
  rt: RuntimeLedger;
  now: () => number;
  constructor(file: string, spec: ToolbeltSpec, rt: RuntimeLedger, now = () => Date.now()) {
    this.file = file; this.spec = spec; this.rt = rt; this.now = now;
    const saved: MeterState | null = existsSync(file) ? JSON.parse(readFileSync(file, "utf8")) : null;
    this.st = saved ?? fresh(now());
    this.st.reserved = "0";
    this.roll();
  }

  roll() {
    const d = dayOf(this.now());
    if (this.st.day !== d) { this.st.day = d; this.st.caps = {}; this.st.minute = {}; }
  }

  save() {
    mkdirSync(dirname(this.file), { recursive: true, mode: 0o700 });
    writeFileSync(`${this.file}.tmp`, JSON.stringify(this.st), { mode: 0o600 });
    renameSync(`${this.file}.tmp`, this.file);
  }

  get balance() { return BigInt(this.st.balance); }
  get free() { return BigInt(this.st.balance) - BigInt(this.st.reserved); }
  day(cap: string): CapDay { return (this.st.caps[cap] ??= { lamports: "0", calls: 0, units: {} }); }

  /** Admit a call: every cap checked, the upper bound reserved. Throws ToolRefused. */
  reserve(cap: Capability, lamports: bigint, usd: number, units: Record<string, number> = {}): Reservation {
    this.roll();
    const c = (this.spec as any)[cap];
    if (!c) throw new ToolRefused("capability_off", `the ${cap} capability is not in this agent's spec`);
    const d = this.day(cap);
    const now = this.now();
    if (d.calls + 1 > c.calls_per_day) throw new ToolRefused("calls_per_day", `${cap}: ${c.calls_per_day} calls per day reached`);
    const win = (this.st.minute[cap] ?? []).filter((t) => now - t < 60_000);
    if (win.length + 1 > c.per_minute) throw new ToolRefused("rate", `${cap}: at most ${c.per_minute} calls per minute`);
    if (BigInt(d.lamports) + lamports > BigInt(c.lamports_per_day)) throw new ToolRefused("lamports_per_day", `${cap}: ${lamports} more lamports would pass ${c.lamports_per_day} today`);
    const uc = unitCaps(this.spec, cap);
    for (const [k, v] of Object.entries(units)) if (uc[k] !== undefined && (d.units[k] ?? 0) + v > uc[k]) throw new ToolRefused(`${k}_per_day`, `${cap}: ${k} ${uc[k]} per day reached`);
    if (lamports > this.free) throw new ToolRefused("balance", `toolbelt balance ${this.free} lamports is below the ${lamports} this call may cost; top up first`);
    if (usd > 0 && !this.rt.admits(usd, now)) throw new ToolRefused("runtime_cap", "the hosted runtime's daily provider spend cap is reached for all agents; try again after 00:00 UTC");
    this.st.reserved = (BigInt(this.st.reserved) + lamports).toString();
    this.rt.reserved += usd;
    this.st.minute[cap] = [...win, now];
    return { cap, lamports, usd, units, t: now };
  }

  /** Book the real cost (at most the reservation, so a cap passed by a bad estimate cannot pass the bound). */
  commit(r: Reservation, lamports: bigint, usd: number, units: Record<string, number> = r.units) {
    const lam = lamports > r.lamports ? r.lamports : lamports < 0n ? 0n : lamports;
    this.st.reserved = (BigInt(this.st.reserved) - r.lamports).toString();
    if (BigInt(this.st.reserved) < 0n) this.st.reserved = "0";
    this.st.balance = (BigInt(this.st.balance) - lam).toString();
    this.st.drawn = (BigInt(this.st.drawn) + lam).toString();
    const d = this.day(r.cap);
    d.lamports = (BigInt(d.lamports) + lam).toString();
    d.calls += 1;
    for (const [k, v] of Object.entries(units)) d.units[k] = (d.units[k] ?? 0) + v;
    this.rt.reserved = Math.max(0, this.rt.reserved - r.usd);
    this.rt.usd += Math.max(0, usd);
    this.rt.save();
    this.save();
    return lam;
  }

  release(r: Reservation) {
    this.st.reserved = (BigInt(this.st.reserved) - r.lamports).toString();
    if (BigInt(this.st.reserved) < 0n) this.st.reserved = "0";
    this.rt.reserved = Math.max(0, this.rt.reserved - r.usd);
  }

  credit(t: Topup) {
    if (this.st.topups.some((x) => x.signature === t.signature)) return false;
    this.st.topups.push(t);
    this.st.topups = this.st.topups.slice(-200);
    this.st.balance = (BigInt(this.st.balance) + BigInt(t.lamports)).toString();
    this.st.credited = (BigInt(this.st.credited) + BigInt(t.lamports)).toString();
    this.save();
    return true;
  }

  nextSeq() { const s = this.st.seq; this.st.seq += 1; return s; }

  status() {
    this.roll();
    return {
      balance_lamports: this.st.balance, credited_lamports: this.st.credited, drawn_lamports: this.st.drawn, day: this.st.day,
      today: Object.fromEntries(Object.entries(this.st.caps).map(([k, v]) => [k, { ...v, lamports_cap: (this.spec as any)[k]?.lamports_per_day ?? "0", calls_cap: (this.spec as any)[k]?.calls_per_day ?? 0 }])),
      topups: this.st.topups.slice(-10), next_seq: this.st.seq,
    };
  }
}
