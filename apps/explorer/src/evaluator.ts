// Evaluator job index: the jobs that name an account among their evaluators (SPEC 4.1),
// with the RejectResults the explorer has indexed for each. The node has no such index
// (listJobs filters by status only), so a wallet's "Jobs I evaluate" would otherwise scan
// every job. Built from the jobs the node lists (all of them, paged by the indexer) and the
// indexed blocks; rebuilt when either changes.
//
// Status filter: the job's own status (open | delivered | completed | expired), or
// `rejected` = jobs with at least one indexed RejectResult (a reject reopens the job, so
// such a job is usually open again, or expired), or `all`.

import type { Indexer } from "./indexer.ts";

export const EVALUATOR_STATUSES = ["all", "open", "delivered", "completed", "expired", "rejected"] as const;
export type EvaluatorStatus = (typeof EVALUATOR_STATUSES)[number];

interface RejectRef {
  height: number;
  deliverer: string;
  result_hash: string;
  signers: string[];
}
interface EvalIndex {
  polls: number;
  blocks: number;
  jobs: number;
  byEvaluator: Map<string, any[]>;
  rejects: Map<string, RejectRef[]>;
}

const cache = new WeakMap<Indexer, EvalIndex>();

export function evaluatorIndex(ix: Indexer): EvalIndex {
  const c = cache.get(ix);
  if (c && c.polls === ix.status.polls && c.blocks === ix.blocks.size && c.jobs === ix.jobs.size) return c;
  const byEvaluator = new Map<string, any[]>();
  const jobs = [...ix.jobs.values()].sort((a, b) => b.posted_height - a.posted_height || b.index - a.index);
  for (const j of jobs) {
    for (const e of new Set<string>(j.evaluators ?? [])) {
      const l = byEvaluator.get(e) ?? [];
      l.push(j);
      byEvaluator.set(e, l);
    }
  }
  const rejects = new Map<string, RejectRef[]>();
  for (const h of [...ix.blocks.keys()].sort((a, b) => a - b)) {
    for (const t of ix.blocks.get(h)!.txs) {
      const f = t.facts ?? {};
      if (t.kind !== "RejectResult" || !f.jobId) continue;
      const l = rejects.get(f.jobId) ?? [];
      l.push({ height: h, deliverer: f.deliverer ?? "", result_hash: f.resultHash ?? "", signers: f.signers ?? [] });
      rejects.set(f.jobId, l);
    }
  }
  const out = { polls: ix.status.polls, blocks: ix.blocks.size, jobs: ix.jobs.size, byEvaluator, rejects };
  cache.set(ix, out);
  return out;
}

/** Jobs naming `account` as an evaluator, newest first, with per-status counts. */
export function evaluatorJobsModel(ix: Indexer, account: string, status: EvaluatorStatus = "all") {
  const idx = evaluatorIndex(ix);
  const all = idx.byEvaluator.get(account) ?? [];
  const counts: Record<EvaluatorStatus, number> = { all: all.length, open: 0, delivered: 0, completed: 0, expired: 0, rejected: 0 };
  for (const j of all) {
    if (j.status in counts) counts[j.status as EvaluatorStatus]++;
    if (idx.rejects.has(j.job_id)) counts.rejected++;
  }
  const pick = status === "all" ? all : status === "rejected" ? all.filter((j) => idx.rejects.has(j.job_id)) : all.filter((j) => j.status === status);
  return {
    evaluator: account,
    status,
    counts,
    jobs: pick.map((j) => {
      const rj = idx.rejects.get(j.job_id) ?? [];
      return {
        job_id: j.job_id,
        status: j.status,
        kind: j.kind ?? "job",
        poster: j.poster,
        worker_pay: j.worker_pay,
        protocol_fee: j.protocol_fee,
        evaluators: j.evaluators,
        threshold: j.threshold,
        deliverer: j.deliverer ?? null,
        result_hash: j.result_hash ?? null,
        result_uri: j.result_uri ?? "",
        posted_height: j.posted_height,
        deadline_height: j.deadline_height,
        closed_height: j.closed_height,
        rejects_indexed: rj.length,
        last_reject: rj.length ? rj[rj.length - 1] : null,
      };
    }),
    indexed: { jobs: ix.jobs.size, blocks: ix.blocks.size, note: "every job the node lists; rejects from the blocks the explorer has indexed" },
  };
}
