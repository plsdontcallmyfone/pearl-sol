// Routes for agent profiles: /agents, /agent/{address} and their /api twins. Node reads
// at query time: getAgent (name, uri) and getRejectStats (the chain's reject tally); the
// agent card is probed by agentcard.ts. Everything else comes from the index (profile.ts).

import type { Indexer } from "./indexer.ts";
import type { Summary } from "./models.ts";
import { CardProber } from "./agentcard.ts";
import { agentCandidates, agentProfile, agentTables, rankKey } from "./profile.ts";
import { agentPage, agentsPage } from "./profile-view.ts";
import { notFound } from "./views.ts";

type Res = { status: number; type: string; body: string | Buffer; headers?: Record<string, string> };
const json = (v: unknown, status = 200): Res => ({ status, type: "application/json; charset=utf-8", body: JSON.stringify(v, null, 1) });
const html = (body: string, status = 200): Res => ({ status, type: "text/html; charset=utf-8", body });

const probers = new WeakMap<Indexer, CardProber>();
export function prober(ix: Indexer): CardProber {
  let p = probers.get(ix);
  if (!p) probers.set(ix, (p = new CardProber()));
  return p;
}

const B58 = /^[1-9A-HJ-NP-Za-km-z]{32,44}$/;

async function nodeReads(ix: Indexer, addr: string) {
  const [agent, rs] = await ix.node.batch<any>([
    ["getAgent", { account: addr }],
    ["getRejectStats", { account: addr }],
  ]).catch((e) => [e, e]);
  return {
    agent: agent instanceof Error ? undefined : agent ?? null,
    rejectStats: rs instanceof Error ? undefined : rs ? { rejected: rs.rejected, rejected_fee: rs.rejected_fee, first_height: rs.first_height, last_height: rs.last_height } : null,
  };
}

/** Full profile with node reads and the card probe. */
export async function fullProfile(ix: Indexer, addr: string) {
  const nr = await nodeReads(ix, addr);
  const p0 = agentProfile(ix, addr, nr);
  const uri = p0.agent?.uri || null;
  const pr = prober(ix);
  const st = await pr.probe(addr, uri);
  const listed = pr.listed(st, p0.server.last_delivery?.time ?? null);
  const card = { uri, health: pr.health(st), listed: listed.listed, listed_reason: listed.reason, ...(st ? { checked_at: st.checked_at, last_ok_at: st.last_ok_at, error: st.error, card: st.card } : {}) };
  return { ...p0, card };
}

/** Ranked list; stale profiles (no fresh card and no recent delivery) hidden unless `all`. */
export async function agentsModel(ix: Indexer, all: boolean) {
  const cands = [...agentCandidates(ix)];
  const t = agentTables(ix);
  const pr = prober(ix);
  const rows: any[] = [];
  // probe due cards, 8 at a time (each bounded by CARD_TIMEOUT_MS)
  const uris = new Map<string, string | null>();
  for (const a of cands) uris.set(a, t.registered.get(a)?.uri || null);
  for (let i = 0; i < cands.length; i += 8) await Promise.all(cands.slice(i, i + 8).map((a) => pr.probe(a, uris.get(a) ?? null)));
  let hidden = 0;
  for (const a of cands) {
    const p = agentProfile(ix, a);
    const st = pr.get(a);
    const l = pr.listed(st, p.server.last_delivery?.time ?? null);
    if (!l.listed) hidden++;
    if (!l.listed && !all) continue;
    rows.push({
      account: a,
      name: p.agent?.name ?? null,
      independent_p: p.server.independent_evaluation.protocol_fees,
      p_behind_completed_work: p.headline.p_behind_completed_work,
      storefront_sol_behind_completed_work: p.headline.storefront_sol_behind_completed_work,
      distinct_payers: p.server.distinct_payers,
      top_payer_share_ppm: p.server.top_payer?.share_ppm ?? null,
      acceptance_by_p_ppm: p.server.acceptance_by_p_ppm,
      rejected_p: p.server.rejected.protocol_fees,
      delivered_then_expired_p: p.server.delivered_then_expired.protocol_fees,
      last_delivery: p.server.last_delivery,
      card_health: pr.health(st),
      listed: l.listed,
      listed_reason: l.reason,
      _k: rankKey(p),
      _k2: BigInt(p.headline.p_behind_completed_work),
    });
  }
  rows.sort((x, y) => (y._k > x._k ? 1 : y._k < x._k ? -1 : y._k2 > x._k2 ? 1 : y._k2 < x._k2 ? -1 : 0));
  for (const r of rows) {
    delete r._k;
    delete r._k2;
  }
  return {
    ranking: "protocol fees P behind completed work, self-dealing excluded, signed by an evaluator other than the job's buyer and seller; never by count",
    hidden_stale: hidden,
    showing_all: all,
    agents: rows,
  };
}

export async function agentRoute(ix: Indexer, api: boolean, rest: string[], q: URLSearchParams, sm: Summary): Promise<Res | null> {
  if (rest[0] === "agents") {
    const m = await agentsModel(ix, q.get("all") === "1");
    return api ? json(m) : html(agentsPage(sm, m));
  }
  if (rest[0] === "agent") {
    const a = rest[1] ?? "";
    if (!B58.test(a)) return api ? json({ error: "not a base58 address" }, 400) : html(notFound(sm, "Agent"), 404);
    const p = await fullProfile(ix, a);
    return api ? json(p) : html(agentPage(sm, p));
  }
  return null;
}
