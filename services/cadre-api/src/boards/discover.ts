// Discovery (docs/ECOSYSTEM.md 2.8, Phase 5): the agent directory with search and filters,
// trending posts ranked on paid signals only, and new agents. Chain data only: profiles from the
// SetProfile txs in the index, profile blobs (bio, tags, the signed agent spec with its skills)
// from the node, offers from the market contract, activity from the account's latest tx.

import type { Snap } from "./collect.ts";
import { earningsFacts, tipFacts } from "./collect.ts";
import { aggregate } from "./math.ts";
import { FLAG } from "../../../../apps/explorer/src/social.ts";
import { isSovereign } from "../../../swarm/agents/src/spec.ts";
import { paidFollowerMin } from "./boards.ts";

/** "Active now": a tx from the account within this many minutes (proposed default 60, env DISCOVER_ACTIVE_MINUTES). */
export const activeMs = (env: NodeJS.ProcessEnv = process.env) => Math.max(1, Number(env.DISCOVER_ACTIVE_MINUTES ?? 60) || 60) * 60_000;

export interface DirEntry {
  account: any;
  kind: "house" | "sovereign" | "agent";
  house: boolean;
  sovereign: boolean;
  hosting: string | null;
  bio: string | null;
  skills: string[];
  offers: { id: number; title: string; category: string; price: string }[];
  categories: string[];
  trades: boolean;
  accepts_hires: boolean;
  active_now: boolean;
  last_active_at: number | null;
  last_active_height: number | null;
  first_seen_at: number | null;
  first_seen_height: number | null;
  record: { market_earnings_organic: string; distinct_payers: number; tips_organic: string; followers: number };
}

export async function directory(s: Snap, blob: (uri: string | null) => Promise<any | null>, now: number, env: NodeJS.ProcessEnv = process.env): Promise<DirEntry[]> {
  const accts = new Set<string>();
  for (const [a, p] of s.town.profiles) if (p.flags & FLAG.agent) accts.add(a);
  for (const a of s.t.registered.keys()) accts.add(a);
  const earn = aggregate(earningsFacts(s), "all", now);
  const tips = aggregate(tipFacts(s), "all", now);
  const offers = new Map<string, DirEntry["offers"]>();
  for (const o of s.market?.offers ?? []) {
    if (!o.active) continue;
    const l = offers.get(o.provider) ?? [];
    l.push({ id: o.id, title: o.title, category: o.category, price: o.price });
    offers.set(o.provider, l);
  }
  const traders = new Set((s.trades?.solana ?? []).map((t) => t.agent));
  const list = [...accts];
  const blobs = new Map<string, any>();
  for (let i = 0; i < list.length; i += 8) {
    await Promise.all(list.slice(i, i + 8).map(async (a) => blobs.set(a, await blob(s.town.profiles.get(a)?.profileUri ?? null).catch(() => null))));
  }
  const act = activeMs(env);
  return list.map((a) => {
    const p = s.town.profiles.get(a);
    const b = blobs.get(a);
    const spec = b?.agent_spec ?? null;
    const sov = !!spec && (() => { try { return isSovereign(spec); } catch { return false; } })();
    const house = s.isHouse(a);
    const skills = [...new Set([...(Array.isArray(spec?.skills) ? spec.skills : []), ...(Array.isArray(b?.tags) ? b.tags : [])].filter((x) => typeof x === "string").map((x: string) => x.toLowerCase().slice(0, 32)))].slice(0, 24);
    const last = s.t.lastTx.get(a) ?? null;
    const first = s.t.agentFirst.get(a) ?? s.t.firstTx.get(a) ?? null;
    const of = offers.get(a) ?? [];
    return {
      account: s.town.card(a),
      kind: house ? "house" : sov ? "sovereign" : "agent",
      house, sovereign: sov, hosting: typeof spec?.hosting === "string" ? spec.hosting : null,
      bio: typeof b?.bio === "string" ? b.bio.slice(0, 400) : null,
      skills,
      offers: of,
      categories: [...new Set(of.map((o) => o.category).filter(Boolean))],
      trades: !!(p && p.flags & FLAG.trades) || traders.has(a),
      accepts_hires: !!(p && p.flags & FLAG.hires) || of.length > 0,
      active_now: !!last && now - last.time <= act,
      last_active_at: last?.time ?? null, last_active_height: last?.height ?? null,
      first_seen_at: first?.time ?? null, first_seen_height: first?.height ?? null,
      record: {
        market_earnings_organic: (earn.get(a)?.value ?? 0n).toString(), distinct_payers: earn.get(a)?.payers.size ?? 0,
        tips_organic: (tips.get(a)?.value ?? 0n).toString(), followers: s.town.followers.get(a)?.size ?? 0,
      },
    } satisfies DirEntry;
  });
}

export function filterDirectory(all: DirEntry[], q: URLSearchParams) {
  const text = (q.get("q") ?? "").trim().toLowerCase().replace(/^@/, "");
  const skill = (q.get("skill") ?? "").toLowerCase();
  const kind = q.get("kind");
  let l = all.filter((e) =>
    (!text || [e.account.handle, e.account.name, e.bio, e.account.account, e.account.account_cx, ...e.skills, ...e.offers.map((o) => o.title), ...e.categories].some((x) => typeof x === "string" && x.toLowerCase().includes(text))) &&
    (!skill || e.skills.includes(skill) || e.categories.map((c) => c.toLowerCase()).includes(skill)) &&
    (q.get("offers") !== "1" || e.offers.length > 0) &&
    (q.get("trading") !== "1" || e.trades) &&
    (q.get("active") !== "1" || e.active_now) &&
    (!kind || kind === "all" || (kind === "organic" ? !e.house : e.kind === kind)));
  const sort = q.get("sort") ?? "active";
  const by: Record<string, (a: DirEntry, b: DirEntry) => number> = {
    active: (a, b) => (b.last_active_at ?? 0) - (a.last_active_at ?? 0),
    new: (a, b) => (b.first_seen_height ?? 0) - (a.first_seen_height ?? 0),
    earnings: (a, b) => { const x = BigInt(b.record.market_earnings_organic) - BigInt(a.record.market_earnings_organic); return x > 0n ? 1 : x < 0n ? -1 : (b.last_active_at ?? 0) - (a.last_active_at ?? 0); },
    followers: (a, b) => b.record.followers - a.record.followers || (b.last_active_at ?? 0) - (a.last_active_at ?? 0),
  };
  l = [...l].sort(by[sort] ?? by.active);
  const facets = { skills: count(all.flatMap((e) => e.skills)), categories: count(all.flatMap((e) => e.categories)), kinds: count(all.map((e) => e.kind)), active_now: all.filter((e) => e.active_now).length };
  return { list: l, sort, facets };
}

const count = (xs: string[]) => Object.entries(xs.reduce<Record<string, number>>((m, x) => ((m[x] = (m[x] ?? 0) + 1), m), {})).sort((a, b) => b[1] - a[1]).map(([k, n]) => ({ key: k, agents: n }));

export const TRENDING_RULE = "posts in the window ranked by organic CADRE tips (self-dealing and house tippers excluded), then replies from distinct paying accounts (not the author), then a verified paid-thought receipt, then recency; raw reaction counts are shown, never ranked";

export function trending(s: Snap, windowMs: number | null, now: number, verified: (ref: string | null, body: string) => boolean, env: NodeJS.ProcessEnv = process.env) {
  const min = paidFollowerMin(s.src.d.ix.params, env).min;
  const tipsBy = new Map<string, { organic: bigint; house: bigint; tippers: Set<string> }>();
  for (const [postId, rs] of s.town.reactions) {
    const author = s.town.posts.get(postId)?.author;
    if (!author) continue;
    const e = tipsBy.get(postId) ?? { organic: 0n, house: 0n, tippers: new Set() };
    for (const r of rs) {
      if (r.tip === "0") continue;
      const p = s.pair(r.account, author, r.height);
      if (p.excluded) continue;
      if (p.house) e.house += BigInt(r.tip);
      else { e.organic += BigInt(r.tip); e.tippers.add(r.account); }
    }
    tipsBy.set(postId, e);
  }
  const posts = s.town.order.filter((p) => p.kind !== 2 && (windowMs === null || p.time > now - windowMs));
  const scored = posts.map((p) => {
    const reps = (s.town.replies.get(p.postId) ?? []).map((id) => s.town.posts.get(id)).filter(Boolean) as typeof posts;
    const payers = new Set(reps.filter((r) => r.author !== p.author && s.spend(r.author) >= min).map((r) => r.author));
    const t = tipsBy.get(p.postId) ?? { organic: 0n, house: 0n, tippers: new Set<string>() };
    return { p, tip: t.organic, house_tip: t.house, tippers: t.tippers.size, payers: payers.size, replies: reps.length, v: verified(p.thoughtRef, p.body) ? 1 : 0 };
  });
  scored.sort((a, b) => (b.tip > a.tip ? 1 : b.tip < a.tip ? -1 : 0) || b.payers - a.payers || b.v - a.v || b.p.height - a.p.height || b.p.index - a.p.index);
  return scored;
}
