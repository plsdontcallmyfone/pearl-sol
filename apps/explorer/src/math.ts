// Pure arithmetic over RPC reads. Everything the explorer reports as a total or a check
// is computed here (unit tested in test/math.test.ts). Amounts are bigint base units:
// chain token 9 dp, SOL lamports 9 dp, Solana SPL token 6 dp. No price source exists,
// so nothing is converted to USD.

export const CHAIN_DP = 9;
export const SOL_DP = 9;
export const SPL_DP = 6;

export const big = (x: string | number | bigint | null | undefined) => (x === null || x === undefined || x === "" ? 0n : BigInt(x));

/** "1234567.891" style decimal with thousands separators; `frac` = max fraction digits (trailing zeros trimmed). */
export function formatUnits(v: bigint | string, decimals: number, frac = 4): string {
  let x = typeof v === "bigint" ? v : BigInt(v);
  const neg = x < 0n;
  if (neg) x = -x;
  const base = 10n ** BigInt(decimals);
  let whole = x / base;
  let rem = x % base;
  // round half up to `frac` digits
  if (frac < decimals) {
    const drop = 10n ** BigInt(decimals - frac);
    let r = rem / drop;
    if ((rem % drop) * 2n >= drop) r += 1n;
    if (r >= 10n ** BigInt(frac)) {
      whole += 1n;
      r = 0n;
    }
    rem = r;
  } else {
    rem = rem * 10n ** BigInt(frac - decimals);
  }
  let f = frac > 0 ? rem.toString().padStart(frac, "0").replace(/0+$/, "") : "";
  const w = whole.toString().replace(/\B(?=(\d{3})+(?!\d))/g, ",");
  if (x !== 0n && whole === 0n && f === "") {
    // nonzero but below the shown precision
    return `${neg ? "-" : ""}<0.${"0".repeat(Math.max(frac - 1, 0))}1`;
  }
  return `${neg ? "-" : ""}${w}${f ? "." + f : ""}`;
}

export const fmtInt = (n: number | bigint) => n.toString().replace(/\B(?=(\d{3})+(?!\d))/g, ",");

/** Share as a percentage string with enough digits to not round a nonzero share to 0. */
export function pct(part: number | bigint, whole: number | bigint): string {
  const p = Number(part);
  const w = Number(whole);
  if (w <= 0) return "0%";
  const v = (p / w) * 100;
  if (v === 0) return "0%";
  if (v === 100) return "100%";
  for (const d of [1, 2, 3, 4, 5, 6]) {
    const s = v.toFixed(d);
    if (Number(s) !== 0 && !(v < 100 && s === "100." + "0".repeat(d))) return `${s.replace(/\.?0+$/, "")}%`;
  }
  return `<0.000001%`;
}

/** mul_ppm exactly as chain/executor (floor of x * ppm / 1e6). */
export const mulPpm = (x: bigint, ppm: bigint | number) => (x * BigInt(ppm)) / 1_000_000n;

export interface JobLike {
  job_id: string;
  poster: string;
  status: string;
  worker_pay: string;
  protocol_fee: string;
  subsidy_paid: string;
  cap_remaining: string;
  blocks_mined: number;
  max_blocks: number;
  posted_height: number;
  /** "job" | "ticket" | "segment" (SPEC 4.5; absent on older nodes = "job"). */
  kind?: string;
  /** Ticket: model_id, cap_total (alpha x P), subsidy_paid over its segments; segment: ticket_id, model_id, matrix. */
  standing?: any;
}

export interface CapCheck {
  /** alpha x P as fixed when the job was posted: subsidy_paid + cap_remaining. */
  cap: bigint;
  /** mul_ppm(P, alpha_ppm) under the current params (equals cap unless alpha changed since). */
  capAtCurrentAlpha: bigint | null;
  subsidyPaid: bigint;
  P: bigint;
  /** subsidy_paid <= cap and cap <= P: the self-dealing bound holds. */
  ok: boolean;
  /** subsidy_paid / P in ppm (what share of the fee came back as subsidy). */
  subsidyPpmOfP: bigint;
}

export function capCheck(job: JobLike, alphaPpm?: number | null): CapCheck {
  const P = big(job.protocol_fee);
  // an inference ticket's subsidy is earned by its segments under cap_total = alpha x P
  const ticket = job.kind === "ticket" && job.standing;
  const paid = ticket ? big(job.standing.subsidy_paid) : big(job.subsidy_paid);
  const cap = ticket ? big(job.standing.cap_total) : paid + big(job.cap_remaining);
  return {
    cap,
    capAtCurrentAlpha: alphaPpm === undefined || alphaPpm === null ? null : mulPpm(P, alphaPpm),
    subsidyPaid: paid,
    P,
    ok: paid <= cap && cap <= P,
    subsidyPpmOfP: P === 0n ? 0n : (paid * 1_000_000n) / P,
  };
}

/** Per job block: the job's cumulative subsidy after each of its blocks (block order), checked against the cap. */
export function cumulativeSubsidy(blocks: { height: number; subsidy: string }[], cap: bigint) {
  let run = 0n;
  return [...blocks]
    .sort((a, b) => a.height - b.height)
    .map((b) => {
      run += big(b.subsidy);
      return { height: b.height, subsidy: big(b.subsidy), cumulative: run, withinCap: run <= cap };
    });
}

export interface BlockLike {
  height: number;
  jobId: string | null; // null = fallback
  fees: string;
  subsidy: string;
}

export interface OrderLike {
  nonce: string;
  status: string;
  buyer: string;
  recipient: string;
  solSpent: string;
  tokens: string;
  protocolFee: string;
  workerPay: string;
  jobId: string;
}

export interface Totals {
  blocks: { total: number; paid: number; fallback: number; paidShare: string; indexedFrom: number; indexedTo: number };
  storefront: {
    orders: number;
    byStatus: Record<string, number>;
    solSettled: bigint; // finalized orders
    solPending: bigint;
    solExpired: bigint;
    solRefunded: bigint; // tokens went back to the buyer; the SOL had bought them
    tokensBoughtSpl: bigint; // all non-refunded orders
  };
  fees: {
    protocolFeesPaid: bigint; // sum of P over chain jobs (non-refundable, into the mining pool)
    workerPayEscrowed: bigint;
    subsidyPaid: bigint; // sum over job blocks
    subsidyFromJobs: bigint; // sum of jobs' subsidy_paid (should equal the above when the index covers every job block)
    blockFees: bigint; // tx fees in indexed blocks
    subsidyPpmOfFees: bigint;
    jobsOverCap: number;
  };
  payers: {
    unique: number; // distinct job posters (each paid W + P)
    uniqueSolanaBuyers: number; // distinct Order.buyer (signed and paid buy_job)
    viaStorefront: number; // distinct posters whose jobs came through the storefront
    topPayer: string | null;
    topPayerShare: string; // of P
    top10Share: string;
  };
  jobs: { total: number; byStatus: Record<string, number>; storefront: number; direct: number };
  /** Inference tickets and their standing jobs (SPEC 4.5); segments are not counted in jobs. */
  inference: { tickets: number; segments: number; segmentBlocks: number };
}

/**
 * Totals over the indexed blocks, the chain's jobs and the storefront's Orders.
 * `orderJobIds` = job ids that came from a storefront order (the rest were posted directly with PostJob).
 */
export function computeTotals(blocks: BlockLike[], jobs: JobLike[], orders: OrderLike[], orderJobIds: Set<string>): Totals {
  let paid = 0;
  let subsidy = 0n;
  let blockFees = 0n;
  let lo = Infinity;
  let hi = -Infinity;
  for (const b of blocks) {
    if (b.height === 0) continue; // genesis
    if (b.jobId) paid++;
    subsidy += big(b.subsidy);
    blockFees += big(b.fees);
    lo = Math.min(lo, b.height);
    hi = Math.max(hi, b.height);
  }
  const total = blocks.filter((b) => b.height !== 0).length;

  const byStatus: Record<string, number> = {};
  let solSettled = 0n, solPending = 0n, solExpired = 0n, solRefunded = 0n, tokensBought = 0n;
  const buyers = new Set<string>();
  for (const o of orders) {
    byStatus[o.status] = (byStatus[o.status] ?? 0) + 1;
    const s = big(o.solSpent);
    if (o.status === "finalized") solSettled += s;
    else if (o.status === "pending") solPending += s;
    else if (o.status === "expired") solExpired += s;
    else if (o.status === "refunded") solRefunded += s;
    if (o.status !== "refunded") {
      tokensBought += big(o.tokens);
      buyers.add(o.buyer);
    }
  }

  let P = 0n, W = 0n, subsidyFromJobs = 0n, overCap = 0, storefront = 0;
  const jobStatus: Record<string, number> = {};
  const pByPoster = new Map<string, bigint>();
  const sfPosters = new Set<string>();
  let tickets = 0, segments = 0, segmentBlocks = 0;
  for (const j of jobs) {
    if (j.kind === "segment") {
      // a standing job of a ticket: W = P = 0, its subsidy counts on the ticket
      segments++;
      segmentBlocks += j.blocks_mined;
      continue;
    }
    if (j.kind === "ticket") tickets++;
    const p = big(j.protocol_fee);
    P += p;
    W += big(j.worker_pay);
    subsidyFromJobs += j.kind === "ticket" ? big(j.standing?.subsidy_paid ?? 0) : big(j.subsidy_paid);
    if (!capCheck(j).ok) overCap++;
    jobStatus[j.status] = (jobStatus[j.status] ?? 0) + 1;
    pByPoster.set(j.poster, (pByPoster.get(j.poster) ?? 0n) + p);
    if (orderJobIds.has(j.job_id)) {
      storefront++;
      sfPosters.add(j.poster);
    }
  }
  const ranked = [...pByPoster.entries()].sort((a, b) => (b[1] > a[1] ? 1 : b[1] < a[1] ? -1 : 0));
  const top10 = ranked.slice(0, 10).reduce((s, [, v]) => s + v, 0n);

  return {
    blocks: {
      total,
      paid,
      fallback: total - paid,
      paidShare: pct(paid, total),
      indexedFrom: lo === Infinity ? 0 : lo,
      indexedTo: hi === -Infinity ? 0 : hi,
    },
    storefront: { orders: orders.length, byStatus, solSettled, solPending, solExpired, solRefunded, tokensBoughtSpl: tokensBought },
    fees: {
      protocolFeesPaid: P,
      workerPayEscrowed: W,
      subsidyPaid: subsidy,
      subsidyFromJobs,
      blockFees,
      subsidyPpmOfFees: P === 0n ? 0n : (subsidy * 1_000_000n) / P,
      jobsOverCap: overCap,
    },
    payers: {
      unique: pByPoster.size,
      uniqueSolanaBuyers: buyers.size,
      viaStorefront: sfPosters.size,
      topPayer: ranked[0]?.[0] ?? null,
      topPayerShare: P === 0n ? "0%" : pct(Number((ranked[0]?.[1] ?? 0n) * 1_000_000n / P), 1_000_000),
      top10Share: P === 0n ? "0%" : pct(Number((top10 * 1_000_000n) / P), 1_000_000),
    },
    jobs: { total: jobs.length - segments, byStatus: jobStatus, storefront, direct: jobs.length - segments - storefront },
    inference: { tickets, segments, segmentBlocks },
  };
}

/** Run-length groups for the feed: consecutive fallback blocks collapse into one row. */
export type FeedRow<B> = { kind: "block"; block: B } | { kind: "fallbackRun"; from: number; to: number; count: number; fees: bigint; blocks: B[] };

export function groupFeed<B extends BlockLike>(blocksNewestFirst: B[], minRun = 3): FeedRow<B>[] {
  const out: FeedRow<B>[] = [];
  let run: B[] = [];
  const flush = () => {
    if (run.length >= minRun) {
      out.push({
        kind: "fallbackRun",
        from: run[run.length - 1].height,
        to: run[0].height,
        count: run.length,
        fees: run.reduce((s, b) => s + big(b.fees), 0n),
        blocks: run,
      });
    } else for (const b of run) out.push({ kind: "block", block: b });
    run = [];
  };
  for (const b of blocksNewestFirst) {
    if (!b.jobId) {
      if (run.length && run[run.length - 1].height !== b.height + 1) flush();
      run.push(b);
    } else {
      flush();
      out.push({ kind: "block", block: b });
    }
  }
  flush();
  return out;
}

export type LcState =
  | { state: "none"; label: string }
  | { state: "before-checkpoint"; label: string }
  | { state: "pending"; label: string }
  | { state: "verified"; label: string }
  | { state: "final"; label: string }
  | { state: "accumulated"; label: string }
  | { state: "not-relayed"; label: string; behind: number }
  | { state: "no-record"; label: string };

export interface LcView {
  checkpointHeight: number;
  tipHeight: number;
  finalHeight: number;
}

/**
 * The Solana light client's verdict on one canonical block, from its HeaderRecord (if the
 * account exists) and the final-header accumulator range. `canonicalFinal` = the node
 * reports the block final on the canonical chain.
 */
export function lcStatus(
  height: number,
  canonicalFinal: boolean,
  lc: LcView | null,
  acc: { startHeight: number; count: number } | null,
  record: { verified: boolean; final: boolean } | null,
): LcState {
  if (!lc) return { state: "none", label: "No light client on this Solana cluster" };
  if (record) {
    if (!record.verified) return { state: "pending", label: "Header submitted, proof being verified" };
    if (record.final) return { state: "final", label: "Proof verified on Solana, header final" };
    return { state: "verified", label: "Proof verified on Solana, not final yet" };
  }
  if (height < lc.checkpointHeight) return { state: "before-checkpoint", label: "Below the light client's trusted checkpoint" };
  if (acc && canonicalFinal && height >= acc.startHeight && height < acc.startHeight + acc.count)
    return { state: "accumulated", label: "Verified and final on Solana (record closed, kept in the accumulator)" };
  if (height > lc.tipHeight) return { state: "not-relayed", label: `Not relayed yet (${height - lc.tipHeight} above the light client tip)`, behind: height - lc.tipHeight };
  return { state: "no-record", label: "No header record on Solana" };
}
