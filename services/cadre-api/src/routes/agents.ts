// Agent profiles (research/agents/REPUTATION.md 5.1): paid work weighted by P and SOL
// from distinct payers, self-dealing shown and excluded, delivered-then-expired as the
// failure signal, mining record. Never a job-count leaderboard: lists sort by screened P
// or SOL, and every count is shown next to the money behind it.
//
// When the agent-layer module lands in the explorer (apps/explorer/src/agents.ts or
// agent.ts exporting agentModel(ix, account) / agentsModel(ix)), these routes serve it
// and say so (`source`); until then they compute the same REPUTATION 5.1 subset here
// from the shared index.

import * as M from "../../../../apps/explorer/src/models.ts";
import { big } from "../../../../apps/explorer/src/math.ts";
import { Api, badRequest } from "../http.ts";
import type { Deps } from "../deps.ts";
import { optionalExport } from "../optional.ts";
import { minerStats } from "./mining.ts";
import { intParam } from "./stats.ts";
import { addr } from "./wallet.ts";

const s = (v: bigint) => v.toString();
const pct = (a: bigint, b: bigint) => (b === 0n ? null : Number((a * 1_000_000n) / b));

function quantile(xs: number[], q: number): number | null {
  if (!xs.length) return null;
  const a = [...xs].sort((x, y) => x - y);
  return a[Math.min(a.length - 1, Math.floor(q * (a.length - 1) + 0.5))];
}

/** The work a key delivered, as server (tickets) or deliverer (plain jobs). */
export function profile(d: Deps, account: string) {
  const ix = d.ix;
  const ord = M.orderByJob(ix);
  const jobs = [...ix.jobs.values()].filter((j) => j.kind !== "segment");
  const delivered = jobs.filter((j) => j.deliverer === account || (j.kind === "ticket" && j.standing?.server === account));
  const completed = delivered.filter((j) => j.status === "completed");
  const expired = delivered.filter((j) => j.status === "expired");
  const self = (j: any) => j.poster === account;
  let pDone = 0n, wDone = 0n, solDone = 0n, pSelf = 0n, pExpired = 0n;
  const byPayer = new Map<string, bigint>();
  const ttc: number[] = [];
  const models = new Map<string, { tickets: number; protocol_fees: bigint }>();
  let firstDelivery: number | null = null, lastDelivery: number | null = null;
  for (const j of completed) {
    const p = big(j.protocol_fee);
    if (self(j)) {
      pSelf += p;
      continue;
    }
    pDone += p;
    wDone += big(j.worker_pay);
    const o = ord.get(j.job_id);
    if (o && o.status === "finalized") solDone += big(o.solSpent);
    byPayer.set(j.poster, (byPayer.get(j.poster) ?? 0n) + p);
    if (j.closed_height > 0) ttc.push(j.closed_height - j.posted_height);
    if (j.kind === "ticket" && j.standing?.model_id) {
      const m = models.get(j.standing.model_id) ?? { tickets: 0, protocol_fees: 0n };
      m.tickets++;
      m.protocol_fees += p;
      models.set(j.standing.model_id, m);
    }
  }
  for (const j of delivered) {
    const h = j.closed_height > 0 ? j.closed_height : j.posted_height;
    firstDelivery = firstDelivery === null ? h : Math.min(firstDelivery, h);
    lastDelivery = lastDelivery === null ? h : Math.max(lastDelivery, h);
  }
  for (const j of expired) if (!self(j)) pExpired += big(j.protocol_fee);
  const top = [...byPayer.values()].reduce((a, b) => (b > a ? b : a), 0n);

  // as buyer
  const posted = jobs.filter((j) => j.poster === account);
  let pPosted = 0n, wPosted = 0n, pPostedDone = 0n, pPostedExpiredNoDelivery = 0n;
  for (const j of posted) {
    const p = big(j.protocol_fee);
    pPosted += p;
    wPosted += big(j.worker_pay);
    if (j.status === "completed") pPostedDone += p;
    if (j.status === "expired" && !j.deliverer) pPostedExpiredNoDelivery += p;
  }

  return {
    account,
    as_server: {
      completed: { count: completed.length - completed.filter(self).length, protocol_fees: s(pDone), worker_pay: s(wDone), sol_settled_via_storefront: s(solDone) },
      distinct_payers: byPayer.size,
      top_payer_share_ppm: pct(top, pDone),
      self_dealt: { count: completed.filter(self).length, protocol_fees: s(pSelf), rule: "completed jobs it posted itself; excluded from every figure above" },
      delivered_then_expired: { count: expired.filter((j) => !self(j)).length, protocol_fees: s(pExpired), note: "a delivery no evaluator quorum accepted before the deadline (an offline evaluator also causes it)" },
      acceptance_ppm_by_fee: pct(pDone, pDone + pExpired),
      blocks_to_completion: { p50: quantile(ttc, 0.5), p90: quantile(ttc, 0.9) },
      models: [...models.entries()].map(([model_id, m]) => ({ model_id, tickets: m.tickets, protocol_fees: s(m.protocol_fees) })),
      first_delivery_height: firstDelivery,
      last_delivery_height: lastDelivery,
      independently_evaluated_share: null,
      independently_evaluated_note: "needs the CompleteJob signer keys (tx body); not in the shared index yet",
    },
    as_buyer: { jobs_posted: posted.length, protocol_fees: s(pPosted), worker_pay: s(wPosted), completed_protocol_fees: s(pPostedDone), expired_without_delivery_protocol_fees: s(pPostedExpiredNoDelivery) },
    as_miner: minerStats(d, account),
  };
}

/** Accounts with a record: deliverers / ticket servers, miners of paid blocks, posters, and RegisterAgent senders. */
function knownAccounts(d: Deps): Set<string> {
  const out = new Set<string>();
  for (const j of d.ix.jobs.values()) {
    if (j.deliverer) out.add(j.deliverer);
    if (j.kind === "ticket" && j.standing?.server) out.add(j.standing.server);
  }
  for (const b of d.ix.blocks.values()) {
    if (b.jobId) out.add(b.miner);
    for (const t of b.txs) if (t.kind === "RegisterAgent") out.add(t.from);
  }
  return out;
}

export function registerAgents(api: Api, d: Deps) {
  api.get("/v1/agents", async (c) => {
    const sort = c.query.get("sort") ?? "paid";
    if (!["paid", "sol", "payers"].includes(sort)) throw badRequest("sort must be paid (screened P), sol (SOL settled) or payers (distinct payers, ties by P)");
    const limit = intParam(c.query, "limit", 50, 1, 200);
    const offset = intParam(c.query, "offset", 0, 0, 1e9);
    const ext = await optionalExport<(ix: unknown, o?: unknown) => any>("../../../apps/explorer/src/agents.ts", ["agentsModel"]);
    if (ext) return { source: "apps/explorer/src/agents.ts", ...(await ext(d.ix, { sort, limit, offset })) };
    const rows = [...knownAccounts(d)].map((a) => profile(d, a));
    const key = (r: ReturnType<typeof profile>) =>
      sort === "sol" ? big(r.as_server.completed.sol_settled_via_storefront) : sort === "payers" ? BigInt(r.as_server.distinct_payers) : big(r.as_server.completed.protocol_fees);
    rows.sort((x, y) => {
      const a = key(x), b = key(y);
      if (a !== b) return b > a ? 1 : -1;
      const pa = big(x.as_server.completed.protocol_fees), pb = big(y.as_server.completed.protocol_fees);
      return pb > pa ? 1 : pb < pa ? -1 : x.account.localeCompare(y.account);
    });
    const withWork = c.query.get("include_idle") === "1" ? rows : rows.filter((r) => r.as_server.completed.count > 0 || r.as_server.delivered_then_expired.count > 0 || r.as_miner.blocks.paid_job > 0);
    const pageRows = withWork.slice(offset, offset + limit);
    const agents = await Promise.all(
      pageRows.map(async (r) => ({
        account: r.account,
        registration: await d.chain<any>("getAgent", { account: r.account }).catch(() => null),
        paid_protocol_fees: r.as_server.completed.protocol_fees,
        sol_settled_via_storefront: r.as_server.completed.sol_settled_via_storefront,
        distinct_payers: r.as_server.distinct_payers,
        top_payer_share_ppm: r.as_server.top_payer_share_ppm,
        completed: r.as_server.completed.count,
        delivered_then_expired: r.as_server.delivered_then_expired.count,
        acceptance_ppm_by_fee: r.as_server.acceptance_ppm_by_fee,
        self_dealt_protocol_fees: r.as_server.self_dealt.protocol_fees,
        paid_job_blocks_mined: r.as_miner.blocks.paid_job,
        last_delivery_height: r.as_server.last_delivery_height,
        profile: `/v1/agents/${r.account}`,
      })),
    );
    return { source: "cadre-api (REPUTATION 5.1 subset over the explorer index)", sort, total: withWork.length, offset, limit, next_offset: offset + limit < withWork.length ? offset + limit : null, agents };
  }, { cache: 10 });

  api.get("/v1/agents/:account", async (c) => {
    const a = addr(c.params.account, "account");
    const ext = await optionalExport<(ix: unknown, a: string) => any>("../../../apps/explorer/src/agents.ts", ["agentModel"]);
    const registration = await d.chain<any>("getAgent", { account: a }).catch(() => null);
    if (ext) return { source: "apps/explorer/src/agents.ts", registration, ...(await ext(d.ix, a)) };
    return { source: "cadre-api (REPUTATION 5.1 subset over the explorer index)", registration, card_uri: registration?.uri || null, ...profile(d, a) };
  }, { cache: 5 });
}
