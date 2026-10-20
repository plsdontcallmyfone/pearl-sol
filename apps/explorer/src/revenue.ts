// Revenue after wash screens (owner decision 2026-10-18, SYNTHESIS item 7, TOKENOMICS 5.3c).
//
// Every figure is a sum over indexed chain jobs (getJob: poster, deliverer, W, P, status),
// the miners of their blocks (getBlock headers) and the storefront Orders on Solana with
// the events of their buy_job tx (OrderPlaced sol_spent + BuybackShareHeld lamports) and
// the storefront Config counters. Nothing is estimated and there is no price source.
//
// Screens (after TRM Labs' agent-payment method, research/agents/TOKENOMICS.md section 4):
//   1. self-dealing, EXCLUDED: the job's payer (chain poster; for a storefront order the
//      x402 payer's key) is also its deliverer or the miner of any block mined on it. That
//      payer gets its own W back and the subsidy, so it is a round trip, not demand.
//   2. concentration, SHOWN: the top payer's and top two payers' share of screened P.
//   3. seller screen, SHOWN NOT EXCLUDED: screened revenue whose deliverer had fewer than 10
//      distinct screened payers (TRM drops such sellers; a young network has few workers,
//      so the explorer reports the amount instead of hiding it).
//   4. house swarm spend, SEPARATE LINE (docs/ECOSYSTEM.md 1.3, M1.5): jobs paid by a key in the
//      chain's `house_operators` (getParams) are the house paying for its own agents' thoughts;
//      they are reported as `house_spend` and never counted in the screened (organic) figures.
// Raw tx counts are never a headline.

import { big } from "./math.ts";
import type { Indexer } from "./indexer.ts";

export interface RevJob {
  job_id: string;
  poster: string;
  deliverer: string | null;
  status: string;
  worker_pay: string;
  protocol_fee: string;
  /** payout keys of the blocks mined on this job */
  miners: string[];
  /** storefront order paying for this job, if any */
  order: { nonce: string; status: string; sol_in: string | null; sol_spent: string } | null;
}

export const SELLER_MIN_PAYERS = 10;

interface Acc {
  jobs: number;
  p: bigint;
  w_delivered: bigint;
  storefront_sol: bigint;
  orders: number;
}
const acc = (): Acc => ({ jobs: 0, p: 0n, w_delivered: 0n, storefront_sol: 0n, orders: 0 });
const s = (v: bigint) => v.toString();
const out = (a: Acc) => ({ jobs: a.jobs, protocol_fees: s(a.p), worker_pay_delivered: s(a.w_delivered), storefront_sol_lamports: s(a.storefront_sol), storefront_orders: a.orders });

/** True when the job's payer also delivered it or mined a block on it. */
export function isSelfDealt(j: RevJob): { self: boolean; reason: string | null } {
  if (j.deliverer && j.deliverer === j.poster) return { self: true, reason: "payer delivered the result" };
  if (j.miners.includes(j.poster)) return { self: true, reason: "payer mined a block on its own job" };
  return { self: false, reason: null };
}

/** SOL a delivered (finalized) storefront order paid in: sol_spent + its buyback share when the buy tx was recorded. */
function orderSol(j: RevJob): bigint {
  const o = j.order;
  if (!o || o.status !== "finalized") return 0n;
  return big(o.sol_in ?? o.sol_spent);
}

function add(a: Acc, j: RevJob) {
  a.jobs++;
  a.p += big(j.protocol_fee);
  if (j.status === "completed") a.w_delivered += big(j.worker_pay);
  const sol = orderSol(j);
  if (sol > 0n) {
    a.storefront_sol += sol;
    a.orders++;
  }
}

export function screenRevenue(jobs: RevJob[], bb: { accrued: string; refunded: string } | null, houseOperators: string[] = []) {
  const gross = acc(), selfDealt = acc(), screened = acc(), sellerShort = acc(), house = acc();
  const houseSet = new Set(houseOperators);
  const housePayers = new Set<string>();
  const payers = new Map<string, bigint>();
  const allPayers = new Set<string>();
  const sellerPayers = new Map<string, Set<string>>();
  const excluded: { job_id: string; payer: string; reason: string; protocol_fee: string }[] = [];
  const kept: RevJob[] = [];
  let unknownSol = 0;
  for (const j of jobs) {
    add(gross, j);
    allPayers.add(j.poster);
    if (j.order?.status === "finalized" && j.order.sol_in === null) unknownSol++;
    if (houseSet.has(j.poster)) {
      add(house, j);
      housePayers.add(j.poster);
      continue;
    }
    const sd = isSelfDealt(j);
    if (sd.self) {
      add(selfDealt, j);
      excluded.push({ job_id: j.job_id, payer: j.poster, reason: sd.reason!, protocol_fee: j.protocol_fee });
      continue;
    }
    add(screened, j);
    kept.push(j);
    payers.set(j.poster, (payers.get(j.poster) ?? 0n) + big(j.protocol_fee));
    if (j.deliverer) {
      const set = sellerPayers.get(j.deliverer) ?? new Set<string>();
      set.add(j.poster);
      sellerPayers.set(j.deliverer, set);
    }
  }
  for (const j of kept) if (j.deliverer && (sellerPayers.get(j.deliverer)?.size ?? 0) < SELLER_MIN_PAYERS) add(sellerShort, j);
  const ranked = [...payers.entries()].sort((a, b) => (b[1] > a[1] ? 1 : b[1] < a[1] ? -1 : 0));
  const ppm = (x: bigint) => (screened.p === 0n ? 0 : Number((x * 1_000_000n) / screened.p));
  return {
    method:
      "Self-dealing excluded: a job whose payer (chain poster) also delivered it or mined a block on it. Shown, not excluded: payer concentration and revenue from deliverers with fewer than 10 distinct payers (TRM Labs' seller screen).",
    gross: { ...out(gross), unique_payers: allPayers.size },
    self_dealt_excluded: { ...out(selfDealt), jobs_list: excluded.slice(0, 50) },
    house_spend: { ...out(house), house_operators: houseOperators, payers: housePayers.size, rule: "jobs whose payer is a declared house operator (chain config house_operators): the house paying for its own agents' thoughts, kept out of the screened figures" },
    screened: {
      ...out(screened),
      unique_payers: payers.size,
      top_payer: ranked[0] ? { payer: ranked[0][0], protocol_fees: s(ranked[0][1]), share_ppm: ppm(ranked[0][1]) } : null,
      top_two_share_ppm: ppm((ranked[0]?.[1] ?? 0n) + (ranked[1]?.[1] ?? 0n)),
      deliverers: sellerPayers.size,
      from_deliverers_under_10_payers: out(sellerShort),
    },
    payers: ranked.slice(0, 20).map(([payer, p]) => ({ payer, protocol_fees: s(p), share_ppm: ppm(p) })),
    // the storefront program's own counters (Config bb_accrued / bb_refunded): released shares are
    // the buyback's SOL, i.e. what holders get (DefiLlama "holders revenue")
    buyback: bb ? { released_shares_lamports: bb.accrued, refunded_shares_lamports: bb.refunded } : null,
    notes: [
      "storefront_sol_lamports counts finalized orders only: the order's sol_spent plus its buyback share (both from the events of its buy_job tx).",
      unknownSol
        ? `${unknownSol} finalized order(s) had their buy_job tx pruned before the explorer saw it: counted at sol_spent without the share.`
        : "Every finalized order's buy_job tx was recorded.",
      "P stays in the mining pool whatever the job's outcome; W counts only for completed jobs (expired jobs return W to the payer).",
    ],
  };
}

/** Builds the screen's input from the index: jobs, their blocks' miners, the paying Orders. */
export function revenueJobs(ix: Indexer): RevJob[] {
  const byJob = new Map<string, any>();
  for (const o of ix.orders.values()) byJob.set(o.jobId, o);
  const out: RevJob[] = [];
  for (const j of ix.jobs.values()) {
    const miners = new Set<string>();
    for (const h of ix.jobBlocks.get(j.job_id) ?? []) {
      const b = ix.blocks.get(h);
      if (b) miners.add(b.miner);
    }
    const o = byJob.get(j.job_id);
    let solIn: string | null = null;
    if (o) {
      for (const t of o.txs ?? []) for (const p of t.paid ?? []) if (p.nonce === o.nonce) solIn = (big(p.solSpent) + big(p.share)).toString();
    }
    out.push({
      job_id: j.job_id,
      poster: j.poster,
      deliverer: j.deliverer ?? null,
      status: j.status,
      worker_pay: j.worker_pay,
      protocol_fee: j.protocol_fee,
      miners: [...miners],
      order: o ? { nonce: o.nonce, status: o.status, sol_in: solIn, sol_spent: o.solSpent } : null,
    });
  }
  return out;
}

export function revenueModel(ix: Indexer) {
  const house: string[] = Array.isArray(ix.params?.house_operators) ? ix.params.house_operators : [];
  return { ...screenRevenue(revenueJobs(ix), ix.buyback ? { accrued: ix.buyback.accrued, refunded: ix.buyback.refunded } : null, house), indexed_blocks: ix.blocks.size };
}
