// Mining: chain parameters, the current per-block subsidy rule, the pool (services/pool)
// when configured, and per-miner earnings from the indexed coinbase data.

import { big } from "../../../../apps/explorer/src/math.ts";
import { Api } from "../http.ts";
import type { Deps } from "../deps.ts";
import { addr } from "./wallet.ts";

const s = (v: bigint) => v.toString();

/** A miner's record from indexed blocks: paid-job vs fallback blocks, subsidy, fees, matured or not. */
export function minerStats(d: Deps, miner: string) {
  const ix = d.ix;
  const tip = ix.chainInfo?.height ?? 0;
  const K = ix.chainInfo?.finality_k ?? 30;
  let paid = 0, fallback = 0, subsidy = 0n, fees = 0n, matured = 0n, immature = 0n, first: number | null = null, last: number | null = null;
  const jobs = new Set<string>(), posters = new Set<string>();
  for (const b of ix.blocks.values()) {
    if (b.miner !== miner || b.height === 0) continue;
    if (b.jobId) {
      paid++;
      jobs.add(b.jobId);
      const j = ix.jobs.get(b.jobId);
      const ticket = j?.kind === "segment" ? ix.jobs.get(j.standing?.ticket_id) : j;
      if (ticket) posters.add(ticket.poster);
    } else fallback++;
    const r = big(b.subsidy) + big(b.fees);
    subsidy += big(b.subsidy);
    fees += big(b.fees);
    if (b.height + K <= tip) matured += r;
    else immature += r;
    first = first === null ? b.height : Math.min(first, b.height);
    last = last === null ? b.height : Math.max(last, b.height);
  }
  return {
    miner,
    blocks: { paid_job: paid, fallback, total: paid + fallback },
    subsidy: s(subsidy),
    fees: s(fees),
    matured: s(matured),
    immature: s(immature),
    maturity_rule: `fees + subsidy are paid to the miner ${K} blocks after the block (coinbase mature_height)`,
    distinct_jobs: jobs.size,
    distinct_posters: posters.size,
    first_block: first,
    last_block: last,
    index_complete: (ix.blocks.size >= tip),
  };
}

export function registerMining(api: Api, d: Deps) {
  api.get("/v1/mining", async () => {
    const [info, params, esc] = await Promise.all([d.chain<any>("getChainInfo", {}), d.chain<any>("getParams", {}), d.chain<any>("getEscrow", {})]);
    const pool = BigInt(esc.pool_balance);
    const perBlock = (pool * BigInt(params.emission_ppm)) / 1_000_000n;
    let poolStats: unknown = null;
    if (d.cfg.poolUrl) {
      const r = await d.upstreamJson("pool", "/stats", { timeoutMs: 5000 }).catch((e) => ({ status: 0, body: { error: (e as Error).message } }));
      poolStats = r.status === 200 ? r.body : { error: r.body?.error ?? `pool HTTP ${r.status}` };
    }
    return {
      chain: { height: info.height, finalized_height: info.finalized_height, target_bits: info.target_bits, next_target_bits: info.next_target_bits, finality_k: info.finality_k },
      params: { target_block_time_ms: params.target_block_time_ms, emission_ppm: params.emission_ppm, alpha_ppm: params.alpha_ppm, min_fee: params.min_fee, fallback_m: params.fallback_m, fallback_n: params.fallback_n, max_job_blocks: params.max_job_blocks },
      mining_pool: { account: esc.pool, balance: esc.pool_balance },
      subsidy_now: { max_per_block: s(perBlock), rule: "a paid-job block earns min(pool x emission_ppm / 1e6, job.cap_remaining); a fallback block earns its tx fees only" },
      pool: d.cfg.poolUrl ? { stats: poolStats, endpoints: { work: "POST /work {payout}", share: "POST /share", miner: "GET /miner?payout=" }, note: "the pool's own HTTP API (services/pool); this API shows its stats only" } : null,
    };
  }, { cache: 5 });

  api.get("/v1/miners/:addr", async (c) => {
    const a = addr(c.params.addr);
    const out: Record<string, unknown> = { ...minerStats(d, a) };
    if (d.cfg.poolUrl) {
      const r = await d.upstreamJson("pool", `/miner?payout=${encodeURIComponent(a)}`, { timeoutMs: 5000 }).catch(() => null);
      out.pool = r && r.status === 200 ? r.body : null;
    }
    return out;
  }, { cache: 5 });
}
