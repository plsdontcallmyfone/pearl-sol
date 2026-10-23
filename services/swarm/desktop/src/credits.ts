// The vision model's credit gate. When the OpenRouter account behind the desktops runs out of credit
// (2026-10-22 22:24 UTC on box 0: every task failed with "model 402: This request requires more
// credits, or fewer max_tokens. You requested up to 400 tokens, but can only afford 137", 571 failed
// tasks in 5 h), retrying is pointless: no task can succeed until someone adds credit. So the first
// credit error PAUSES all model-driven work (vision tasks, and the swarm's chain actions, which would
// pay for a Cadre thought the desktop cannot then type), with one log line and a status flag. While
// paused, a cheap probe runs every DESK_CREDIT_PROBE_S; the first probe that shows credit again
// resumes everything, with one log line. Credit can come back at any time (the owner tops up), so
// nothing here assumes the account stays empty.
//
// The probe, cheapest first:
//   1. GET /api/v1/key (free, no tokens): a per-key limit (`limit_remaining`) or an org budget
//      (`effective_budget.remaining_usd`) under DESK_CREDIT_MIN_USD means still empty, no spend.
//      An account with no per-key limit reports null there, which proves nothing either way.
//   2. One tiny chat request ("Reply with: ok") with max_tokens DESK_CREDIT_PROBE_MAX_TOKENS. OpenRouter
//      checks affordability against max_tokens before it runs the request, so the probe only passes
//      when the account can afford that many output tokens; the reply itself is a few tokens.
//      The default (1600) is a real task step's 400 output tokens plus about 4,000 prompt tokens
//      (a screenshot step), counted at the list prices in vision.ts (0.10 / 0.50 USD per million in /
//      out, so 4,000 prompt tokens cost the same as 800 output tokens) and rounded up.
// (/api/v1/credits would give the account balance, but it needs a management key; the desktops'
// key is an ordinary one, so it is not used.)
import type { Chat } from "./vision.ts";

export interface CreditError {
  status: number;
  /** "can only afford N" (output tokens) when the provider says so */
  afford: number | null;
  /** "You requested up to N tokens" */
  requested: number | null;
  /** short provider message, no secrets (OpenRouter's text) */
  message: string;
}

const CREDIT_TEXT = /requires more credits|can only afford|insufficient (credits|balance|funds)|prompt tokens limit exceeded|add more credits|key limit exceeded|out of credits|credit limit/i;

/** Is this model response an out-of-credit refusal? (402 always; 403 or other codes only with a credit
 * message: OpenRouter answers "Key limit exceeded" with 403.) Null when it is any other error. */
export function creditError(status: number, json: any): CreditError | null {
  if (status === 200) return null;
  const msg = String(json?.error?.message ?? json?.message ?? (typeof json?.error === "string" ? json.error : "") ?? "");
  if (status !== 402 && !CREDIT_TEXT.test(msg)) return null;
  const afford = /can only afford (\d+)/i.exec(msg);
  const req = /requested up to (\d+)/i.exec(msg);
  return { status, afford: afford ? Number(afford[1]) : null, requested: req ? Number(req[1]) : null, message: msg.replace(/\s+/g, " ").slice(0, 160) };
}

export interface ProbeResult {
  ok: boolean;
  /** USD the probe cost (OpenRouter's usage.cost), booked as vision spend */
  usd: number;
  /** why it did or did not pass, one short phrase */
  why: string;
  afford?: number | null;
}

export interface ProbeDeps {
  /** GET /api/v1/key; may be absent (tests, other providers) */
  keyInfo?: () => Promise<{ status: number; json: any }>;
  chat: Chat;
  model: string;
  maxTokens: number;
  minUsd: number;
}

/** One credit probe (see the header). Inconclusive answers (5xx, a dropped connection) count as not ok:
 * the gate stays paused and probes again at the next interval. */
export async function probeCredits(d: ProbeDeps): Promise<ProbeResult> {
  if (d.keyInfo) {
    const k = await d.keyInfo().catch(() => null);
    if (k?.status === 401) return { ok: false, usd: 0, why: "key rejected (401)" };
    const data = k?.status === 200 ? k.json?.data : null;
    const lim = typeof data?.limit_remaining === "number" ? data.limit_remaining : null;
    if (lim !== null && lim < d.minUsd) return { ok: false, usd: 0, why: `key limit_remaining ${lim} USD` };
    const bud = typeof data?.effective_budget?.remaining_usd === "number" ? data.effective_budget.remaining_usd : null;
    if (bud !== null && bud < d.minUsd) return { ok: false, usd: 0, why: `budget remaining ${bud} USD` };
  }
  const r = await d.chat({ model: d.model, messages: [{ role: "user", content: "Reply with: ok" }], max_tokens: d.maxTokens, usage: { include: true } })
    .catch((e: any) => ({ status: 0, json: { error: { message: String(e?.message ?? e).slice(0, 120) } } }));
  const usd = Number(r.json?.usage?.cost ?? 0) || 0;
  if (r.status === 200 && r.json?.choices?.[0]) return { ok: true, usd, why: `probe answered (max_tokens ${d.maxTokens})` };
  const c = creditError(r.status, r.json);
  if (c) return { ok: false, usd, why: `still out of credit (${c.status}${c.afford !== null ? `, can only afford ${c.afford} tokens` : ""})`, afford: c.afford };
  return { ok: false, usd, why: `probe inconclusive (http ${r.status})` };
}

export interface GateOpts {
  /** seconds between probes while paused */
  probeS: number;
  /** a pause within this many seconds of a resume doubles the probe interval (up to maxProbeS) */
  flapS: number;
  maxProbeS: number;
  /** seconds paused before the desktops themselves are paused (E2B bills running desktops); -1 never */
  holdDesksS: number;
  /** how often a "still paused" line is logged, seconds */
  remindS: number;
  log: (e: Record<string, unknown>) => void;
  now?: () => number;
}

export class CreditGate {
  state: "ok" | "paused" = "ok";
  since = 0;
  reason = "";
  last: CreditError | null = null;
  where = "";
  probes = 0;
  probeUsd = 0;
  lastProbe: (ProbeResult & { at: number }) | null = null;
  nextProbeAt = 0;
  intervalMs: number;
  lastResumeAt = 0;
  lastRemindAt = 0;
  /** vision tasks and chain actions not started while paused */
  skipped = 0;
  /** the desktops were paused by this gate (the controller resumes them on resume) */
  desksHeld = false;
  pauses = 0;
  private o: GateOpts;
  private now: () => number;
  private probing = false;
  constructor(o: GateOpts) {
    this.o = o;
    this.now = o.now ?? (() => Date.now());
    this.intervalMs = o.probeS * 1000;
  }

  get paused() { return this.state === "paused"; }

  /** A model call came back out of credit: pause (one log line). Returns true when this call paused it. */
  trip(e: CreditError, where: string): boolean {
    if (this.paused) return false;
    const t = this.now();
    // flapping: paused again soon after a resume (the probe passed, a real task did not): probe less often
    if (this.lastResumeAt && t - this.lastResumeAt < this.o.flapS * 1000) this.intervalMs = Math.min(this.o.maxProbeS * 1000, this.intervalMs * 2);
    this.state = "paused";
    this.since = t;
    this.last = e;
    this.where = where;
    this.reason = e.afford !== null
      ? `out of credit: the account can only afford ${e.afford} of the ${e.requested ?? "?"} max_tokens a step asks for`
      : `out of credit (${e.status}): ${e.message.slice(0, 100)}`;
    this.nextProbeAt = t + this.intervalMs;
    this.lastRemindAt = t;
    this.pauses++;
    this.o.log({ event: "vision_paused", reason: this.reason, status: e.status, afford: e.afford, requested: e.requested, where, probe_every_s: Math.round(this.intervalMs / 1000), hold_desktops_after_s: this.o.holdDesksS });
    return true;
  }

  /** Counts work that was not started because of the pause. */
  skip() { this.skipped++; }

  probeDue(t = this.now()) { return this.paused && !this.probing && t >= this.nextProbeAt; }

  /** The desktops should be paused now (paused long enough, not already held). */
  holdDue(t = this.now()) { return this.paused && !this.desksHeld && this.o.holdDesksS >= 0 && t - this.since >= this.o.holdDesksS * 1000; }

  /** Runs one probe when due; resumes on success (one log line). Returns true when it resumed. */
  async tick(probe: () => Promise<ProbeResult>): Promise<boolean> {
    const t = this.now();
    if (!this.probeDue(t)) return false;
    this.probing = true;
    let r: ProbeResult;
    try { r = await probe(); } catch (e: any) { r = { ok: false, usd: 0, why: `probe failed: ${String(e?.message ?? e).slice(0, 80)}` }; }
    finally { this.probing = false; }
    const t2 = this.now();
    this.probes++;
    this.probeUsd += r.usd;
    this.lastProbe = { ...r, at: t2 };
    if (r.ok) {
      this.o.log({ event: "vision_resumed", paused_s: Math.round((t2 - this.since) / 1000), probes: this.probes, probe_usd: Number(this.probeUsd.toFixed(6)), skipped: this.skipped, desktops_were_held: this.desksHeld, why: r.why });
      this.state = "ok";
      this.lastResumeAt = t2;
      this.reason = "";
      this.probes = 0; this.probeUsd = 0; this.skipped = 0;
      // desksHeld stays true until the controller has started them again (it clears it)
      return true;
    }
    this.nextProbeAt = t2 + this.intervalMs;
    if (t2 - this.lastRemindAt >= this.o.remindS * 1000) {
      this.lastRemindAt = t2;
      this.o.log({ event: "vision_still_paused", paused_min: Math.round((t2 - this.since) / 60000), probes: this.probes, probe_usd: Number(this.probeUsd.toFixed(6)), skipped: this.skipped, desktops_held: this.desksHeld, last_probe: r.why });
    }
    return false;
  }

  status() {
    const t = this.now();
    return {
      state: this.state,
      paused: this.paused,
      since: this.paused ? new Date(this.since).toISOString() : null,
      paused_s: this.paused ? Math.round((t - this.since) / 1000) : 0,
      reason: this.paused ? this.reason : null,
      afford_tokens: this.paused ? this.last?.afford ?? null : null,
      where: this.paused ? this.where : null,
      probe_every_s: Math.round(this.intervalMs / 1000),
      next_probe_at: this.paused ? new Date(this.nextProbeAt).toISOString() : null,
      probes: this.probes,
      probe_usd: Number(this.probeUsd.toFixed(6)),
      last_probe: this.lastProbe ? { at: new Date(this.lastProbe.at).toISOString(), ok: this.lastProbe.ok, why: this.lastProbe.why } : null,
      skipped: this.skipped,
      desktops_held: this.desksHeld,
      pauses_since_start: this.pauses,
    };
  }
}
