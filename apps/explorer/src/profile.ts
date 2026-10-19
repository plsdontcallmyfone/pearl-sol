// Agent profiles from the index (owner decision 2026-10-18, REPUTATION.md 5.1).
//
// No new chain state beyond RejectResult: every figure is a sum over indexed chain jobs
// (listJobs / getJob), the txs of indexed canonical blocks (SubmitResult, CompleteJob and
// RejectResult signer keys, Transfers, RegisterAgent), coinbase data of indexed blocks
// (getBlock: miner, job_id, fees, subsidy), storefront Orders on Solana (sol_spent plus the
// buyback share from the events of their buy_job tx, revenue.ts) and two node reads at
// query time (getAgent, getRejectStats). Nothing is estimated and there is no price source.
//
// The headline is money behind completed work, never a count: P (the non-refundable
// protocol fee, which a self-dealer loses at least (1 - alpha) x P of) and storefront SOL,
// both with self-dealing excluded. Counts are shown only next to the amount they carry.
// Never shown as reputation (REPUTATION.md 5.1): registration age alone, tx counts, raw job
// counts, raw unique wallets, any rating.

import type { Indexer } from "./indexer.ts";
import { big } from "./math.ts";
import { isSelfDealt, revenueJobs, type RevJob } from "./revenue.ts";

const s = (v: bigint) => v.toString();
const ppm = (part: bigint, whole: bigint) => (whole === 0n ? null : Number((part * 1_000_000n) / whole));

export const SOURCES = {
  p_behind_completed_work: "sum of protocol_fee over listJobs jobs with deliverer = account, status completed, kind job or ticket, self-dealing excluded",
  storefront_sol_behind_completed_work: "Solana Order accounts (status finalized) whose job is one of those: sol_spent + buyback share from the OrderPlaced / BuybackShareHeld events of its buy_job tx (sol_spent alone when that tx was pruned before the explorer saw it)",
  w_earned: "sum of worker_pay over the same completed jobs (credited by CompleteJob)",
  distinct_payers: "distinct poster over those jobs (for a storefront order the poster is the x402 payer's key)",
  top_payer_share: "largest single poster's share of that P",
  self_dealt: "completed jobs whose poster is the deliverer or mined a block on the job (getBlock miner); heuristic, labelled: poster received a Transfer from the deliverer at or before the job's posted height (indexed blocks only)",
  delivered_then_expired: "listJobs jobs with status expired and deliverer = account (a delivery no evaluator quorum accepted before the deadline; an offline evaluator also causes it)",
  rejected: "RejectResult txs in indexed blocks naming the account as deliverer, P from the job; chain tally from getRejectStats",
  acceptance_by_p: "completed P / (completed P + delivered-then-expired P + rejected P), all deliveries, self-dealt included",
  independent_evaluation: "share of completed (screened) P whose CompleteJob tx carried a signature from a key that is neither the job's poster nor its deliverer (signer keys decoded from the CompleteJob tx body)",
  time_to_completion: "closed_height - posted_height of the completed jobs, in blocks",
  time_to_delivery: "height of the account's SubmitResult tx - posted_height, in blocks (indexed blocks only)",
  mining: "indexed blocks with header miner = account: job blocks (job_id set) vs fallback, getBlock fees and subsidy",
  buyer: "listJobs jobs with poster = account",
  evaluator: "CompleteJob and RejectResult txs in indexed blocks that carried the account's signature",
};

export interface AgentTx {
  height: number;
  time: number;
  jobId: string;
  signers: string[];
  from: string;
}

/** Per-index derived tables, rebuilt when the indexer polled since the last build. */
export interface AgentTables {
  polls: number;
  blocks: number;
  completes: Map<string, AgentTx>; // job id -> its CompleteJob
  submits: Map<string, { height: number; time: number; from: string }[]>; // job id -> SubmitResults
  rejects: (AgentTx & { deliverer: string; resultHash: string })[];
  transfers: Map<string, { from: string; height: number; amount: string }[]>; // recipient -> transfers to it
  registered: Map<string, { height: number; name: string; uri: string }>; // last RegisterAgent per sender
  rev: Map<string, RevJob>;
  heightTime: Map<number, number>;
}

const cache = new WeakMap<Indexer, AgentTables>();

export function agentTables(ix: Indexer): AgentTables {
  const c = cache.get(ix);
  if (c && c.polls === ix.status.polls && c.blocks === ix.blocks.size) return c;
  const t: AgentTables = { polls: ix.status.polls, blocks: ix.blocks.size, completes: new Map(), submits: new Map(), rejects: [], transfers: new Map(), registered: new Map(), rev: new Map(), heightTime: new Map() };
  const heights = [...ix.blocks.keys()].sort((a, b) => a - b);
  for (const h of heights) {
    const b = ix.blocks.get(h)!;
    t.heightTime.set(h, b.time);
    for (const x of b.txs) {
      const f = x.facts ?? {};
      switch (x.kind) {
        case "CompleteJob":
          if (f.jobId) t.completes.set(f.jobId, { height: h, time: b.time, jobId: f.jobId, signers: f.signers ?? [], from: x.from });
          break;
        case "SubmitResult":
          if (f.jobId) {
            const l = t.submits.get(f.jobId) ?? [];
            l.push({ height: h, time: b.time, from: x.from });
            t.submits.set(f.jobId, l);
          }
          break;
        case "RejectResult":
          if (f.jobId && f.deliverer) t.rejects.push({ height: h, time: b.time, jobId: f.jobId, signers: f.signers ?? [], from: x.from, deliverer: f.deliverer, resultHash: f.resultHash ?? "" });
          break;
        case "Transfer":
          if (f.to) {
            const l = t.transfers.get(f.to) ?? [];
            l.push({ from: x.from, height: h, amount: f.amount ?? "0" });
            t.transfers.set(f.to, l);
          }
          break;
        case "RegisterAgent":
          if (f.name !== undefined) t.registered.set(x.from, { height: h, name: f.name, uri: f.uri ?? "" });
          break;
      }
    }
  }
  for (const r of revenueJobs(ix)) t.rev.set(r.job_id, r);
  cache.set(ix, t);
  return t;
}

function quantile(xs: number[], q: number): number | null {
  if (xs.length === 0) return null;
  const v = [...xs].sort((a, b) => a - b);
  return v[Math.min(v.length - 1, Math.max(0, Math.ceil(q * v.length) - 1))];
}

const isWork = (j: any) => (j.kind ?? "job") !== "segment";

/** Self-dealing of one job: strict (payer delivered / mined it) or the labelled funding heuristic. */
export function selfDealing(t: AgentTables, j: any): { self: boolean; reason: string | null; heuristic: boolean } {
  const r = t.rev.get(j.job_id);
  if (r) {
    const sd = isSelfDealt(r);
    if (sd.self) return { self: true, reason: sd.reason, heuristic: false };
  } else if (j.deliverer && j.deliverer === j.poster) return { self: true, reason: "payer delivered the result", heuristic: false };
  if (j.deliverer) {
    const fund = (t.transfers.get(j.poster) ?? []).find((x) => x.from === j.deliverer && x.height <= j.posted_height);
    if (fund) return { self: true, reason: `heuristic: the payer was funded by the deliverer (Transfer at #${fund.height})`, heuristic: true };
  }
  return { self: false, reason: null, heuristic: false };
}

/** Storefront SOL a finalized order paid in for this job, or 0. */
function jobSol(t: AgentTables, jobId: string): bigint {
  const o = t.rev.get(jobId)?.order;
  if (!o || o.status !== "finalized") return 0n;
  return big(o.sol_in ?? o.sol_spent);
}

export interface ProfileExtra {
  /** getAgent result, or null when not registered; undefined when the read failed. */
  agent?: { name: string; uri: string; registered_height: number; updated_height: number } | null;
  /** getRejectStats result (chain tally), or undefined when unavailable. */
  rejectStats?: { rejected: number; rejected_fee: string; first_height: number; last_height: number } | null;
  card?: unknown;
}

/** One account's record in every role (pure over the index; node reads come in `extra`). */
export function agentProfile(ix: Indexer, addr: string, extra: ProfileExtra = {}) {
  const t = agentTables(ix);
  const jobs = [...ix.jobs.values()].filter(isWork);

  // ---------------------------------------------------------- as server / deliverer
  const done = jobs.filter((j) => j.deliverer === addr && j.status === "completed");
  const dte = jobs.filter((j) => j.deliverer === addr && j.status === "expired");
  const delivering = jobs.filter((j) => j.deliverer === addr && j.status === "delivered");
  const myRejects = t.rejects.filter((r) => r.deliverer === addr);

  let pScreened = 0n, wScreened = 0n, solScreened = 0n, pGross = 0n, solGross = 0n, wGross = 0n;
  let pSelf = 0n, solSelf = 0n, pHeur = 0n, pIndep = 0n, pSigUnknown = 0n;
  let ordersScreened = 0;
  const payerP = new Map<string, bigint>();
  const selfList: { job_id: string; payer: string; reason: string; protocol_fee: string }[] = [];
  const completed: any[] = [];
  const ttc: number[] = [];
  const ttd: number[] = [];
  for (const j of done) {
    const p = big(j.protocol_fee);
    const sol = jobSol(t, j.job_id);
    pGross += p;
    solGross += sol;
    wGross += big(j.worker_pay);
    const sd = selfDealing(t, j);
    const cj = t.completes.get(j.job_id) ?? null;
    const signers = cj?.signers ?? null;
    const independent = signers === null ? null : signers.some((k) => k !== j.poster && k !== j.deliverer);
    if (sd.self) {
      pSelf += p;
      solSelf += sol;
      if (sd.heuristic) pHeur += p;
      selfList.push({ job_id: j.job_id, payer: j.poster, reason: sd.reason!, protocol_fee: j.protocol_fee });
    } else {
      pScreened += p;
      wScreened += big(j.worker_pay);
      solScreened += sol;
      if (sol > 0n) ordersScreened++;
      payerP.set(j.poster, (payerP.get(j.poster) ?? 0n) + p);
      if (independent === true) pIndep += p;
      if (independent === null) pSigUnknown += p;
    }
    if (j.closed_height > 0) ttc.push(j.closed_height - j.posted_height);
    const sub = (t.submits.get(j.job_id) ?? []).filter((x) => x.from === addr).pop();
    if (sub) ttd.push(sub.height - j.posted_height);
    completed.push({
      job_id: j.job_id, kind: j.kind ?? "job", poster: j.poster, protocol_fee: j.protocol_fee, worker_pay: j.worker_pay,
      storefront_sol_lamports: s(sol), posted_height: j.posted_height, closed_height: j.closed_height,
      self_dealt: sd.self ? sd.reason : null,
      signers, signed_by_third_party: independent, complete_height: cj?.height ?? null,
    });
  }
  completed.sort((a, b) => b.closed_height - a.closed_height);
  const ranked = [...payerP.entries()].sort((a, b) => (b[1] > a[1] ? 1 : b[1] < a[1] ? -1 : 0));
  const pDte = dte.reduce((a, j) => a + big(j.protocol_fee), 0n);
  let pRej = 0n;
  const rejected = myRejects.map((r) => {
    const j = ix.jobs.get(r.jobId);
    const p = big(j?.protocol_fee);
    pRej += p;
    return { job_id: r.jobId, height: r.height, result_hash: r.resultHash, protocol_fee: j ? j.protocol_fee : null, poster: j?.poster ?? null, signers: r.signers, job_status_now: j?.status ?? null };
  });
  const accDen = pGross + pDte + pRej;

  // delivery activity (any SubmitResult it sent, rejected ones included)
  let firstDel: { height: number; time: number } | null = null;
  let lastDel: { height: number; time: number } | null = null;
  for (const l of t.submits.values())
    for (const x of l) {
      if (x.from !== addr) continue;
      if (!firstDel || x.height < firstDel.height) firstDel = { height: x.height, time: x.time };
      if (!lastDel || x.height > lastDel.height) lastDel = { height: x.height, time: x.time };
    }

  // per model (tickets it served)
  const models = new Map<string, { tickets: number; rows: number; protocol_fees: bigint }>();
  for (const j of done) {
    const m = j.standing?.model_id;
    if (j.kind !== "ticket" || !m) continue;
    const r = models.get(m) ?? { tickets: 0, rows: 0, protocol_fees: 0n };
    r.tickets++;
    r.rows += Number(j.standing?.rows ?? 0);
    r.protocol_fees += big(j.protocol_fee);
    models.set(m, r);
  }

  // ---------------------------------------------------------- as miner
  let jobBlocks = 0, fbBlocks = 0, subsidy = 0n, feesJob = 0n, feesFb = 0n;
  const minedJobs = new Set<string>();
  const minedFor = new Set<string>();
  const segModels = new Map<string, number>();
  for (const b of ix.blocks.values()) {
    if (b.miner !== addr) continue;
    if (b.jobId) {
      jobBlocks++;
      subsidy += big(b.subsidy);
      feesJob += big(b.fees);
      minedJobs.add(b.jobId);
      const j = ix.jobs.get(b.jobId);
      if (j) minedFor.add(j.poster);
      const m = j?.kind === "segment" ? j.standing?.model_id : null;
      if (m) segModels.set(m, (segModels.get(m) ?? 0) + 1);
    } else {
      fbBlocks++;
      feesFb += big(b.fees);
    }
  }

  // ---------------------------------------------------------- as buyer
  const posted = jobs.filter((j) => j.poster === addr);
  const bySt = (st: string, pred: (j: any) => boolean = () => true) => posted.filter((j) => j.status === st && pred(j)).reduce((a, j) => a + big(j.protocol_fee), 0n);
  const postedP = posted.reduce((a, j) => a + big(j.protocol_fee), 0n);
  const postedW = posted.reduce((a, j) => a + big(j.worker_pay), 0n);
  const expiredNoDelivery = bySt("expired", (j) => !j.deliverer);

  // ---------------------------------------------------------- as evaluator
  let evJobs = 0, evP = 0n, evConflictP = 0n, evRej = 0, evRejP = 0n;
  const evSellers = new Set<string>(), evBuyers = new Set<string>();
  for (const c of t.completes.values()) {
    if (!c.signers.includes(addr)) continue;
    const j = ix.jobs.get(c.jobId);
    if (!j) continue;
    evJobs++;
    evP += big(j.protocol_fee);
    if (j.deliverer) evSellers.add(j.deliverer);
    evBuyers.add(j.poster);
    if (j.deliverer === addr || j.poster === addr) evConflictP += big(j.protocol_fee);
  }
  for (const r of t.rejects) {
    if (!r.signers.includes(addr)) continue;
    evRej++;
    evRejP += big(ix.jobs.get(r.jobId)?.protocol_fee);
  }

  const reg = extra.agent !== undefined ? extra.agent : (t.registered.get(addr) ? { name: t.registered.get(addr)!.name, uri: t.registered.get(addr)!.uri, registered_height: t.registered.get(addr)!.height, updated_height: t.registered.get(addr)!.height } : null);

  return {
    account: addr,
    agent: reg,
    indexed: { blocks: ix.blocks.size, tip_height: ix.chainInfo?.height ?? null, note: "figures cover the jobs the node lists and the blocks the explorer has indexed" },
    headline: {
      p_behind_completed_work: s(pScreened),
      storefront_sol_behind_completed_work: s(solScreened),
      storefront_orders: ordersScreened,
      w_earned: s(wScreened),
      completed_jobs: done.length - selfList.length,
    },
    server: {
      distinct_payers: payerP.size,
      top_payer: ranked[0] ? { payer: ranked[0][0], protocol_fees: s(ranked[0][1]), share_ppm: ppm(ranked[0][1], pScreened) } : null,
      self_dealt_excluded: {
        jobs: selfList.length,
        protocol_fees: s(pSelf),
        storefront_sol_lamports: s(solSelf),
        share_ppm_of_completed_p: ppm(pSelf, pGross),
        heuristic_protocol_fees: s(pHeur),
        list: selfList.slice(0, 50),
      },
      gross_completed: { jobs: done.length, protocol_fees: s(pGross), worker_pay: s(wGross), storefront_sol_lamports: s(solGross) },
      delivered_then_expired: { jobs: dte.length, protocol_fees: s(pDte) },
      rejected: { jobs: rejected.length, protocol_fees: s(pRej), chain_tally: extra.rejectStats ?? null, list: rejected.slice(-50).reverse() },
      awaiting_evaluation: { jobs: delivering.length, protocol_fees: s(delivering.reduce((a, j) => a + big(j.protocol_fee), 0n)) },
      acceptance_by_p_ppm: ppm(pGross, accDen),
      independent_evaluation: {
        protocol_fees: s(pIndep),
        share_ppm: ppm(pIndep, pScreened),
        signers_not_indexed_protocol_fees: s(pSigUnknown),
        note: "independent = at least one CompleteJob signer is neither the job's poster nor its deliverer; keys run by the same operator cannot be told apart from chain state",
      },
      time_to_completion_blocks: { p50: quantile(ttc, 0.5), p90: quantile(ttc, 0.9), n: ttc.length },
      time_to_delivery_blocks: { p50: quantile(ttd, 0.5), p90: quantile(ttd, 0.9), n: ttd.length },
      first_delivery: firstDel,
      last_delivery: lastDel,
      models: [...models.entries()].map(([model_id, r]) => ({ model_id, tickets: r.tickets, rows: r.rows, protocol_fees: s(r.protocol_fees) })),
      completed: completed.slice(0, 100),
    },
    miner: {
      job_blocks: jobBlocks,
      fallback_blocks: fbBlocks,
      subsidy: s(subsidy),
      fees_job_blocks: s(feesJob),
      fees_fallback_blocks: s(feesFb),
      distinct_jobs: minedJobs.size,
      distinct_payers_mined_for: minedFor.size,
      segment_blocks_by_model: [...segModels.entries()].map(([model_id, blocks]) => ({ model_id, blocks })),
    },
    buyer: {
      jobs: posted.length,
      protocol_fees: s(postedP),
      worker_pay: s(postedW),
      completed_p: s(bySt("completed")),
      expired_without_delivery_p: s(expiredNoDelivery),
      expired_without_delivery_share_ppm: ppm(expiredNoDelivery, postedP),
      open_p: s(bySt("open") + bySt("delivered")),
    },
    evaluator: {
      jobs_signed: evJobs,
      protocol_fees: s(evP),
      distinct_sellers: evSellers.size,
      distinct_buyers: evBuyers.size,
      rejects_signed: evRej,
      rejects_signed_p: s(evRejP),
      own_job_share_ppm: ppm(evConflictP, evP),
    },
    card: extra.card ?? null,
    sources: SOURCES,
  };
}

/** Accounts with a record worth listing: a delivery, a mined block, a registration or a signature. */
export function agentCandidates(ix: Indexer): Set<string> {
  const t = agentTables(ix);
  const out = new Set<string>();
  for (const j of ix.jobs.values()) if (j.deliverer && isWork(j)) out.add(j.deliverer);
  for (const r of t.rejects) out.add(r.deliverer);
  for (const a of t.registered.keys()) out.add(a);
  return out;
}

/** Ranking key for lists: P behind completed, screened work signed by a third-party evaluator. */
export function rankKey(p: ReturnType<typeof agentProfile>): bigint {
  return big(p.server.independent_evaluation.protocol_fees);
}
