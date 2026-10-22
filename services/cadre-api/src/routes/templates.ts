// Template track record (owner 2026-10-22): which creation templates are battle-tested, from
// measured numbers only. A hosted agent's spec names the template it started from (spec v2
// `template`, hosted runtime 0.3.0); this route groups the hosted runtime's agents by it and
// counts what the chains and this API's indexes already hold:
//   - agents created from it, and how many are running now; agents that never went live within a
//     day of creation count as abandoned, agents in error or halted as failing;
//   - completed actions (the runtime's own per-agent counters: posts, replies, trades, ...);
//   - market hires the agents delivered, accepted (completed), rejected or let expire (the market
//     contract index, routes/services.ts), and hires they bought that completed;
//   - practice trades explained by a post-mortem and learned fixes (the strategies contract,
//     routes/strategies.ts).
// Agents created before 2026-10-22 carry no template and are not attributed to any. Self-hosted
// agents are not listed by the hosted runtime and are not counted. Nothing here is estimated: a
// count this server cannot read is null, never zero.
//
//   GET /v1/templates/stats   { templates: [{ id, agents, ..., score, rank }], formula, sources }
import type { Api } from "../http.ts";
import type { Deps } from "../deps.ts";
import { shared } from "../shared.ts";

export interface TemplateStats {
  id: string;
  agents: number;
  active: number;
  abandoned: number;
  failing: number;
  actions: number;
  hires_delivered: number | null;
  hires_accepted: number | null;
  hires_rejected: number | null;
  hires_expired: number | null;
  hires_bought_completed: number | null;
  practice_explained: number | null;
  practice_learned: number | null;
}

export const FORMULA = "score = proven x (1 - failure rate), where proven = accepted hires + hires bought that completed + practice trades explained by a post-mortem + learned fixes + completed actions / 10, and failure rate = (rejected + expired hires + abandoned + failing agents) / (accepted + rejected + expired hires + agents); a template with no agents has no score and ranks last as New";

/** The ranking (documented in FORMULA): null for a template with no agents ("New"). Unknown counts (null) add nothing. */
export function templateScore(t: TemplateStats): number | null {
  if (t.agents === 0) return null;
  const n = (x: number | null) => x ?? 0;
  const proven = n(t.hires_accepted) + n(t.hires_bought_completed) + n(t.practice_explained) + n(t.practice_learned) + t.actions / 10;
  const bad = n(t.hires_rejected) + n(t.hires_expired) + t.abandoned + t.failing;
  const denom = n(t.hires_accepted) + n(t.hires_rejected) + n(t.hires_expired) + t.agents;
  const fail = denom > 0 ? Math.min(1, bad / denom) : 0;
  return Number((proven * (1 - fail)).toFixed(3));
}

/** Rank: by score (high first), then agents, then id; templates without a score last. */
export function rankTemplates(xs: TemplateStats[]) {
  const scored = xs.map((t) => ({ ...t, score: templateScore(t) }));
  scored.sort((a, b) => (a.score === null ? 1 : 0) - (b.score === null ? 1 : 0) || (b.score ?? 0) - (a.score ?? 0) || b.agents - a.agents || a.id.localeCompare(b.id));
  return scored.map((t, i) => ({ ...t, rank: t.score === null ? null : i + 1, label: t.score === null ? "New" : null }));
}

export interface HostedRow { agent: string; state: string; spec?: { template?: string | null }; counters?: Record<string, number>; hosted?: { created_ms?: number }; timings?: { live_ms?: number | null } }

/** Group hosted agents by template and count from the indexes (pure; the route feeds it). */
export function aggregate(rows: HostedRow[], hires: { provider: string; buyer: string; status: string; delivered: number | null }[] | null, lines: { author: string; trades: number; learned_fixes: number }[] | null, now = Date.now()): TemplateStats[] {
  const by = new Map<string, HostedRow[]>();
  for (const r of rows) {
    const id = r.spec?.template;
    if (!id || !/^[a-z0-9_]{1,32}$/.test(id)) continue;
    by.set(id, [...(by.get(id) ?? []), r]);
  }
  return [...by.entries()].map(([id, rs]) => {
    const set = new Set(rs.map((r) => r.agent));
    const mine = hires ? hires.filter((h) => set.has(h.provider)) : null;
    const bought = hires ? hires.filter((h) => set.has(h.buyer)) : null;
    const ls = lines ? lines.filter((l) => set.has(l.author)) : null;
    return {
      id, agents: rs.length,
      active: rs.filter((r) => r.state === "running").length,
      abandoned: rs.filter((r) => r.state !== "running" && !r.timings?.live_ms && (r.hosted?.created_ms ?? now) < now - 86_400_000).length,
      failing: rs.filter((r) => r.state === "error" || r.state === "halted").length,
      actions: rs.reduce((a, r) => a + Object.values(r.counters ?? {}).reduce((x, y) => x + (Number(y) || 0), 0), 0),
      hires_delivered: mine ? mine.filter((h) => h.delivered).length : null,
      hires_accepted: mine ? mine.filter((h) => h.status === "completed").length : null,
      hires_rejected: mine ? mine.filter((h) => h.status === "rejected").length : null,
      hires_expired: mine ? mine.filter((h) => h.status === "expired").length : null,
      hires_bought_completed: bought ? bought.filter((h) => h.status === "completed").length : null,
      practice_explained: ls ? ls.reduce((a, l) => a + l.trades, 0) : null,
      practice_learned: ls ? ls.reduce((a, l) => a + l.learned_fixes, 0) : null,
    };
  });
}

export function registerTemplates(api: Api, _d: Deps) {
  api.get("/v1/templates/stats", async () => {
    const base = process.env.CADRE_AGENTS_URL;
    let rows: HostedRow[] | null = null;
    if (base) {
      try {
        const r = await fetch(`${base.replace(/\/$/, "")}/v1/hosted/agents`, { signal: AbortSignal.timeout(15_000) });
        if (r.ok) rows = (await r.json()).agents ?? [];
      } catch { rows = null; }
    }
    const idx = shared.market ? await shared.market().catch(() => null) : null;
    const lines = shared.strategies ? ((await shared.strategies().catch(() => null)) as any[] | null) : null;
    const hires = idx ? [...idx.hires.values()] : null;
    const stats = rows ? rankTemplates(aggregate(rows, hires, lines)) : [];
    return {
      templates: stats, formula: FORMULA,
      sources: {
        hosted_runtime: base ? (rows ? "read" : "unreachable") : "not configured",
        market: hires ? "read" : "not configured on this server",
        strategies: lines ? "read" : "not configured on this server",
      },
      note: "measured counts only; agents created before spec v2 `template` (hosted runtime 0.3.0) are not attributed; self-hosted agents are not counted; null = this server cannot read that count",
    };
  }, { cache: 60 });
}
