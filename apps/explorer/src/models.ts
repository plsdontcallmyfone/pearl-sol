// Page models: plain JSON (amounts as decimal strings of base units) built from the
// indexer. Every HTML page renders one of these and /api serves the same object, so
// agents read exactly what people see.

import * as D from "./decode.ts";
import type { BlockRec, Indexer, OrderRec, SolTx } from "./indexer.ts";
import { big, capCheck, computeTotals, cumulativeSubsidy, groupFeed, lcStatus, type LcState } from "./math.ts";
import { BRAND } from "./brand.ts";

const s = (v: bigint) => v.toString();

export function lcView(ix: Indexer) {
  const c = ix.lc.config;
  return c ? { checkpointHeight: c.checkpointHeight, tipHeight: c.tipHeight, finalHeight: c.finalHeight } : null;
}

function isFinal(ix: Indexer, h: number) {
  return ix.chainInfo ? h <= ix.chainInfo.finalized_height : false;
}

/** Light-client verdicts for many blocks, reading HeaderRecords only where one can still exist. */
export async function lcStatuses(ix: Indexer, blocks: BlockRec[]): Promise<Map<string, LcState & { record?: string }>> {
  const out = new Map<string, LcState & { record?: string }>();
  const c = ix.lc.config;
  const need: BlockRec[] = [];
  for (const b of blocks) {
    if (!c) out.set(b.hash, lcStatus(b.height, false, null, null, null));
    else if (b.height < c.checkpointHeight || b.height > c.tipHeight) out.set(b.hash, lcStatus(b.height, isFinal(ix, b.height), lcView(ix), ix.lc.acc, null));
    else need.push(b);
  }
  if (need.length) {
    let recs: (ReturnType<typeof D.decodeHeaderRecord> | null)[] = need.map(() => null);
    const pdas = need.map((b) => D.headerPda(b.hash));
    try {
      const accs = await ix.sol.getMultipleAccounts(pdas);
      recs = accs.map((a) => (a && a.owner === D.LIGHTCLIENT_ID ? D.decodeHeaderRecord(a.address, a.data) : null));
    } catch {
      /* Solana unreachable: fall back to the accumulator range only */
    }
    need.forEach((b, i) => {
      const r = recs[i];
      out.set(b.hash, { ...lcStatus(b.height, isFinal(ix, b.height), lcView(ix), ix.lc.acc, r), ...(r ? { record: pdas[i] } : {}) });
    });
  }
  return out;
}

export function orderByJob(ix: Indexer) {
  const m = new Map<string, OrderRec>();
  for (const o of ix.orders.values()) m.set(o.jobId, o);
  return m;
}

function finalizeMode(o: OrderRec): { mode: "proof" | "signers" | "unknown"; tx: SolTx | null } {
  for (const t of o.txs) {
    if (t.ix.includes("FinalizeOrderProof") || t.ix.includes("ExpireOrderProof")) return { mode: "proof", tx: t };
    if (t.ix.includes("FinalizeOrder") || t.ix.includes("ExpireOrder")) return { mode: "signers", tx: t };
  }
  return { mode: "unknown", tx: null };
}

function buyTx(o: OrderRec) {
  return o.txs.find((t) => t.ix.includes("BuyJob") || t.ix.includes("BuyJobAmm")) ?? null;
}

export function orderModel(o: OrderRec) {
  const fin = finalizeMode(o);
  return {
    nonce: o.nonce,
    address: o.address,
    status: o.status,
    venue: o.venue,
    job_id: o.jobId,
    buyer: o.buyer,
    payer: o.recipient,
    sol_spent: o.solSpent,
    tokens_spl: o.tokens,
    worker_pay: o.workerPay,
    protocol_fee: o.protocolFee,
    created_slot: o.createdSlot,
    created_ts: o.createdTs,
    closed_slot: o.closedSlot,
    deliverer: o.deliverer,
    result_hash: o.resultHash,
    result_uri: o.resultUri,
    data_uri: o.dataUri,
    shape: { m: o.m, n: o.n, k: o.k, max_blocks: o.maxBlocks, deadline_blocks: o.deadlineBlocks },
    buy_tx: buyTx(o),
    x402_payment: o.payment,
    settlement: { mode: fin.mode, tx: fin.tx },
    refund_tx: o.txs.find((t) => t.ix.includes("MarkRefunded")) ?? null,
    solana_txs: o.txs,
    txs_recorded: o.txs.length > 0,
  };
}

export function summaryModel(ix: Indexer) {
  const blocks = [...ix.blocks.values()];
  const jobs = [...ix.jobs.values()];
  const orders = [...ix.orders.values()];
  const orderJobIds = new Set(orders.filter((o) => o.status !== "refunded").map((o) => o.jobId));
  const t = computeTotals(blocks, jobs, orders, orderJobIds);
  const tip = ix.chainInfo?.height ?? 0;
  const c = ix.lc.config;
  return {
    name: BRAND.name,
    ticker: BRAND.ticker,
    chain: ix.chainInfo
      ? { height: tip, finalized_height: ix.chainInfo.finalized_height, genesis_hash: ix.genesis, finality_k: ix.chainInfo.finality_k, target_block_time_ms: ix.params?.target_block_time_ms ?? null }
      : null,
    params: ix.params ? { alpha_ppm: ix.params.alpha_ppm, emission_ppm: ix.params.emission_ppm, min_fee: ix.params.min_fee } : null,
    index: {
      blocks_indexed: t.blocks.total,
      blocks_on_chain: tip,
      complete: t.blocks.total >= tip,
      backfill_next: ix.backfillCursor,
    },
    blocks: { total: t.blocks.total, paid_job_blocks: t.blocks.paid, fallback_blocks: t.blocks.fallback, paid_share: t.blocks.paidShare },
    storefront: {
      orders: t.storefront.orders,
      by_status: t.storefront.byStatus,
      sol_settled: s(t.storefront.solSettled),
      sol_pending: s(t.storefront.solPending),
      sol_expired: s(t.storefront.solExpired),
      sol_refunded: s(t.storefront.solRefunded),
      tokens_bought_spl: s(t.storefront.tokensBoughtSpl),
    },
    fees: {
      protocol_fees_paid: s(t.fees.protocolFeesPaid),
      protocol_fee_destination: "mining pool (non-refundable; nothing is burned on the chain)",
      worker_pay: s(t.fees.workerPayEscrowed),
      subsidy_paid: s(t.fees.subsidyPaid),
      subsidy_paid_by_jobs: s(t.fees.subsidyFromJobs),
      subsidy_ppm_of_protocol_fees: s(t.fees.subsidyPpmOfFees),
      block_tx_fees: s(t.fees.blockFees),
      jobs_over_cap: t.fees.jobsOverCap,
    },
    payers: {
      unique_payers: t.payers.unique,
      unique_solana_buyers: t.payers.uniqueSolanaBuyers,
      storefront_payers: t.payers.viaStorefront,
      top_payer: t.payers.topPayer,
      top_payer_share_of_fees: t.payers.topPayerShare,
      top10_share_of_fees: t.payers.top10Share,
    },
    jobs: { total: t.jobs.total, by_status: t.jobs.byStatus, via_storefront: t.jobs.storefront, posted_directly: t.jobs.direct },
    light_client: c
      ? {
          checkpoint_height: c.checkpointHeight,
          tip_height: c.tipHeight,
          final_height: c.finalHeight,
          behind_chain: Math.max(0, tip - c.tipHeight),
          accumulated: ix.lc.acc ? { start_height: ix.lc.acc.startHeight, count: ix.lc.acc.count } : null,
        }
      : null,
    withdrawals: {
      count: ix.withdrawals.size,
      released_on_solana: [...ix.withdrawals.values()].filter((w) => w.released).length,
    },
    status: { node_ok: ix.status.nodeOk, solana_ok: ix.status.solOk, node_error: ix.status.nodeError || null, solana_error: ix.status.solError || null, last_poll_ms: ix.status.lastPoll },
  };
}

export type Summary = ReturnType<typeof summaryModel>;

export async function blockRow(ix: Indexer, b: BlockRec, lc: LcState, ord: Map<string, OrderRec>) {
  const job = b.jobId ? ix.jobs.get(b.jobId) : null;
  const o = b.jobId ? ord.get(b.jobId) : undefined;
  let cap: ReturnType<typeof capCheck> | null = null;
  let cumulative: bigint | null = null;
  if (job) {
    cap = capCheck(job, ix.params?.alpha_ppm);
    const hs = ix.jobBlocks.get(job.job_id) ?? [];
    const rows = cumulativeSubsidy(hs.map((h) => ({ height: h, subsidy: ix.blocks.get(h)!.subsidy })), cap.cap);
    cumulative = rows.find((r) => r.height === b.height)?.cumulative ?? null;
  }
  return {
    height: b.height,
    hash: b.hash,
    time_ms: b.time,
    miner: b.miner,
    kind: b.jobId ? "paid_job" : "fallback",
    job_id: b.jobId,
    paid_by: job ? job.poster : null,
    paid_via: job ? (o && o.status !== "refunded" ? "storefront" : "post_job") : null,
    order_nonce: o ? o.nonce : null,
    sol_spent: o ? o.solSpent : null,
    worker_pay: job ? job.worker_pay : null,
    protocol_fee: job ? job.protocol_fee : null,
    subsidy: b.subsidy,
    fees: b.fees,
    reward: b.reward,
    subsidy_cap: cap ? s(cap.cap) : null,
    job_subsidy_after_block: cumulative === null ? null : s(cumulative),
    within_cap: cap ? cumulative !== null && cumulative <= cap.cap && cap.ok : null,
    proof_hash: b.proofHash,
    proof_len: b.proofLen,
    tx_count: b.txs.length,
    light_client: lc,
  };
}

export type BlockRow = Awaited<ReturnType<typeof blockRow>>;

export async function feedModel(ix: Indexer, opts: { filter: "all" | "paid" | "fallback"; before?: number; limit?: number }) {
  const limit = Math.min(Math.max(opts.limit ?? 40, 1), 200);
  const tip = ix.chainInfo?.height ?? 0;
  let start = opts.before && opts.before > 0 ? opts.before - 1 : tip;
  const picked: BlockRec[] = [];
  if (opts.filter === "paid") {
    const hs = [...ix.jobBlocks.values()].flat().filter((h) => h <= start).sort((a, b) => b - a);
    for (const h of hs.slice(0, limit)) picked.push(ix.blocks.get(h)!);
  } else {
    // "all": newest first; a run of fallback blocks counts as one row, so read further back
    let rows = 0;
    let lastWasFallback = false;
    for (let h = start; h >= 1 && picked.length < 5000; h--) {
      const b = ix.blocks.get(h);
      if (!b) continue;
      if (opts.filter === "fallback" && b.jobId) continue;
      if (opts.filter === "all") {
        if (b.jobId) {
          rows++;
          lastWasFallback = false;
        } else if (!lastWasFallback) {
          rows++;
          lastWasFallback = true;
        }
        if (rows > limit) break;
      } else if (picked.length >= limit) break;
      picked.push(b);
    }
  }
  const ord = orderByJob(ix);
  const groups = opts.filter === "all" ? groupFeed(picked, 2) : picked.map((b) => ({ kind: "block" as const, block: b }));
  const shown: BlockRec[] = [];
  for (const g of groups) {
    if (g.kind === "block") shown.push(g.block);
    else shown.push(g.blocks[0], g.blocks[g.blocks.length - 1]);
  }
  const lcMap = await lcStatuses(ix, shown);
  const row = (b: BlockRec) => blockRow(ix, b, lcMap.get(b.hash)!, ord);
  const rows = [];
  for (const g of groups) {
    if (g.kind === "block") rows.push({ kind: "block" as const, block: await row(g.block) });
    else
      rows.push({
        kind: "fallback_run" as const,
        from: g.from,
        to: g.to,
        count: g.count,
        fees: s(g.fees),
        newest: await row(g.blocks[0]),
        oldest: await row(g.blocks[g.blocks.length - 1]),
      });
  }
  const grouped = rows;
  const oldest = picked.length ? picked[picked.length - 1].height : null;
  return { filter: opts.filter, before: opts.before ?? null, tip, rows: grouped, next_before: oldest && oldest > 1 ? oldest : null };
}

export async function blockModel(ix: Indexer, id: string) {
  let b: BlockRec | undefined;
  if (/^\d+$/.test(id)) b = ix.blocks.get(Number(id));
  else b = ix.blocks.get(ix.byHash.get(id.toLowerCase()) ?? -1);
  let raw: any = null;
  try {
    raw = await ix.node.call("getBlock", /^\d+$/.test(id) ? { height: Number(id) } : { hash: id.toLowerCase() });
  } catch {
    /* node down: serve the index copy */
  }
  if (!raw && !b) return null;
  const { toBlockRec } = await import("./indexer.ts");
  if (raw) b = toBlockRec(raw);
  const blk = b!;
  const lc = (await lcStatuses(ix, [blk])).get(blk.hash)!;
  const row = await blockRow(ix, blk, lc, orderByJob(ix));
  // light-client txs: captured ones, else whatever the Solana ledger still has
  let lcTxs = ix.lcTxs.get(blk.hash) ?? null;
  if (!lcTxs && ix.lc.config && blk.height >= ix.lc.config.checkpointHeight && blk.height <= ix.lc.config.tipHeight) {
    const list: SolTx[] = [];
    try {
      await ix.captureTxs(D.headerPda(blk.hash), D.LIGHTCLIENT_ID, list);
      lcTxs = list;
    } catch {
      lcTxs = null;
    }
  }
  const job = blk.jobId ? ix.jobs.get(blk.jobId) ?? null : null;
  const o = blk.jobId ? orderByJob(ix).get(blk.jobId) : undefined;
  const withdrawals = [...ix.withdrawals.values()]
    .filter((w) => w.height === blk.height)
    .map((w) => ({ ...w, release_mode: releaseMode(w.releaseTxs) }));
  return {
    ...row,
    prev_hash: blk.prev,
    target_bits: blk.targetBits,
    canonical: raw ? raw.canonical : true,
    final: raw ? raw.final : isFinal(ix, blk.height),
    confirmations: raw ? raw.confirmations : null,
    coinbase: raw?.coinbase ?? null,
    header: raw?.header ?? null,
    header_fields: raw?.header_fields ?? null,
    matured: raw?.matured ?? null,
    job: job ? jobSummary(ix, job) : null,
    order: o ? orderModel(o) : null,
    settlement_wait: settlementWait(ix, o),
    txs: blk.txs.map((t) => ({ ...t, links: txLinks(ix, t) })),
    light_client_header_pda: D.headerPda(blk.hash),
    light_client_txs: lcTxs,
    light_client_txs_note: lcTxs === null ? "not recorded: outside the light client's range or the Solana RPC's ledger window" : null,
    withdrawals,
    proof_raw: `/api/block/${blk.height}/proof`,
  };
}

function releaseMode(txs: SolTx[]) {
  for (const t of txs) {
    if (t.ix.includes("ReleaseProof")) return { mode: "proof", tx: t };
    if (t.ix.includes("Release")) return { mode: "signers", tx: t };
  }
  return { mode: "unknown", tx: null };
}

function txLinks(ix: Indexer, t: { kind: string; facts: D.TxFacts }) {
  const f = t.facts;
  if (f.jobId) return { job: f.jobId };
  if (f.orderJobId) return { job: f.orderJobId, order: f.solanaNonce };
  return {};
}

function jobSummary(ix: Indexer, j: any) {
  const cap = capCheck(j, ix.params?.alpha_ppm);
  return {
    job_id: j.job_id,
    status: j.status,
    poster: j.poster,
    worker_pay: j.worker_pay,
    protocol_fee: j.protocol_fee,
    subsidy_paid: j.subsidy_paid,
    subsidy_cap: s(cap.cap),
    cap_at_current_alpha: cap.capAtCurrentAlpha === null ? null : s(cap.capAtCurrentAlpha),
    within_cap: cap.ok,
    subsidy_ppm_of_fee: s(cap.subsidyPpmOfP),
    blocks_mined: j.blocks_mined,
    max_blocks: j.max_blocks,
    m: j.m,
    n: j.n,
    k: j.k,
    posted_height: j.posted_height,
    closed_height: j.closed_height,
    deadline_height: j.deadline_height,
  };
}

export async function jobModel(ix: Indexer, id: string) {
  const key = id.toLowerCase();
  let j = ix.jobs.get(key);
  if (!j && key.length >= 6) j = [...ix.jobs.values()].find((x) => x.job_id.startsWith(key));
  if (!j) return null;
  const o = orderByJob(ix).get(j.job_id);
  const hs = ix.jobBlocks.get(j.job_id) ?? [];
  const blocks = hs.map((h) => ix.blocks.get(h)!);
  const lcMap = await lcStatuses(ix, blocks);
  const cap = capCheck(j, ix.params?.alpha_ppm);
  const cum = cumulativeSubsidy(blocks.map((b) => ({ height: b.height, subsidy: b.subsidy })), cap.cap);
  const chainTxs = ix.jobTxs(j).map(({ height, tx }) => ({ height, ...tx }));
  return {
    ...jobSummary(ix, j),
    paid_via: o && o.status !== "refunded" ? "storefront" : "post_job",
    data_uri: j.data_uri,
    hash_a: j.hash_a,
    hash_b: j.hash_b,
    job_key: j.job_key,
    deliverer: j.deliverer,
    result_hash: j.result_hash,
    result_uri: j.result_uri,
    evaluators: j.evaluators,
    threshold: j.threshold,
    blocks: blocks.map((b, i) => ({
      height: b.height,
      hash: b.hash,
      time_ms: b.time,
      miner: b.miner,
      subsidy: b.subsidy,
      job_subsidy_after_block: s(cum[i].cumulative),
      within_cap: cum[i].withinCap,
      proof_hash: b.proofHash,
      light_client: lcMap.get(b.hash)!,
      light_client_txs: ix.lcTxs.get(b.hash) ?? null,
    })),
    blocks_indexed: blocks.length,
    chain_txs: chainTxs,
    order: o ? orderModel(o) : null,
    settlement_wait: settlementWait(ix, o),
  };
}

export function jobsModel(ix: Indexer) {
  const ord = orderByJob(ix);
  const jobs = [...ix.jobs.values()].sort((a, b) => b.posted_height - a.posted_height || b.index - a.index);
  return {
    total: jobs.length,
    jobs: jobs.map((j) => {
      const o = ord.get(j.job_id);
      return { ...jobSummary(ix, j), paid_via: o && o.status !== "refunded" ? "storefront" : "post_job", order_nonce: o?.nonce ?? null, sol_spent: o?.solSpent ?? null };
    }),
  };
}

export function ordersModel(ix: Indexer) {
  const orders = [...ix.orders.values()].sort((a, b) => Number(big(b.nonce) - big(a.nonce)));
  return { total: orders.length, orders: orders.map((o) => ({ ...orderModel(o), on_chain_job: ix.jobs.has(o.jobId) })) };
}

export function orderPageModel(ix: Indexer, nonce: string) {
  const o = ix.orders.get(nonce);
  if (!o) return null;
  return { ...orderModel(o), on_chain_job: ix.jobs.has(o.jobId), settlement_wait: settlementWait(ix, o), job: ix.jobs.has(o.jobId) ? jobSummary(ix, ix.jobs.get(o.jobId)) : null };
}

/** When a pending Order can settle by proof: the light client's final height must reach the job's close (CompleteJob / ExpireJob). */
export function settlementWait(ix: Indexer, o: OrderRec | undefined | null) {
  if (!o || o.status !== "pending") return null;
  const j = ix.jobs.get(o.jobId);
  if (!j || !(j.closed_height > 0)) return null;
  const fin = ix.lc.config?.finalHeight ?? null;
  return { closed_height: j.closed_height, job_status: j.status, light_client_final: fin, blocks_to_go: fin === null ? null : Math.max(0, j.closed_height - fin) };
}

/** Payers ranked by protocol fees paid (buyer concentration, so self-dealing shows). */
export function payersModel(ix: Indexer) {
  const ord = orderByJob(ix);
  const by = new Map<string, { payer: string; jobs: number; storefront_jobs: number; protocol_fees: bigint; sol_spent: bigint }>();
  let total = 0n;
  for (const j of ix.jobs.values()) {
    const r = by.get(j.poster) ?? { payer: j.poster, jobs: 0, storefront_jobs: 0, protocol_fees: 0n, sol_spent: 0n };
    r.jobs++;
    const o = ord.get(j.job_id);
    if (o && o.status !== "refunded") {
      r.storefront_jobs++;
      r.sol_spent += big(o.solSpent);
    }
    r.protocol_fees += big(j.protocol_fee);
    total += big(j.protocol_fee);
    by.set(j.poster, r);
  }
  const rows = [...by.values()].sort((a, b) => (b.protocol_fees > a.protocol_fees ? 1 : b.protocol_fees < a.protocol_fees ? -1 : 0));
  return {
    total_protocol_fees: s(total),
    payers: rows.map((r) => ({
      payer: r.payer,
      jobs: r.jobs,
      storefront_jobs: r.storefront_jobs,
      protocol_fees: s(r.protocol_fees),
      sol_spent: s(r.sol_spent),
      share_ppm: total === 0n ? 0 : Number((r.protocol_fees * 1_000_000n) / total),
    })),
  };
}
